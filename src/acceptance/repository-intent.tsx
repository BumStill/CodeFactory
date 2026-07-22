// SPDX-License-Identifier: Apache-2.0
// Lock-safe acceptance entry for repository-owned specifications and
// conversation-native execution. It mounts the real WorkspacePage and remote
// Issue panel against bounded Tauri IPC fixtures.

import React from "react";
import { createRoot } from "react-dom/client";
import { mockIPC, mockWindows } from "@tauri-apps/api/mocks";

import "../styles/globals.css";
import type { GitRemoteConfig, RemoteIssue, Session, Settings, TaskRun } from "../lib/tauri";

const sessionId = "repository-intent-session";
const cwd = "/repo-owned-intent-fixture";
const session: Session = {
  id: sessionId,
  title: "repository-owned-intent",
  cwd,
  model_id: "anthropic/claude-opus-4-7",
  created_at: 1,
  updated_at: 1,
  total_input_tokens: 0,
  total_output_tokens: 0,
  kind: "project",
};
const delegatedTask: TaskRun = {
  id: "delegated-task",
  session_id: sessionId,
  title: "在会话内执行仓库需求",
  description: "Repository intent is executed inside this conversation.",
  status: "pending",
  cwd,
  parent_task_id: null,
  sub_session_id: null,
  created_at: "2026-07-22T00:00:00Z",
  started_at: null,
  completed_at: null,
  result: null,
  error: null,
  attempt_count: 0,
  verification_results: null,
  task_context_json: null,
  acceptance_criteria_json: null,
  spec_req_id: null,
  spec_title: null,
} as TaskRun;
const remote: GitRemoteConfig = {
  id: "github",
  name: "GitHub",
  provider: "github",
  base_url: "https://api.github.com",
  default_repo: "BumStill/CodeFactory",
  has_token: true,
};
const issue: RemoteIssue = {
  id: 101,
  number: 101,
  title: "Repository-owned specification",
  body: "Keep durable product intent in ordinary versioned repository files.",
  state: "open",
  labels: ["product"],
  created_at: "2026-07-22T00:00:00Z",
  updated_at: "2026-07-22T00:00:00Z",
  url: "https://github.com/BumStill/CodeFactory/issues/101",
  author: "fixture",
};

mockWindows("main");
mockIPC(
  (command) => {
    switch (command) {
      case "list_sessions": return [session];
      case "list_quick_sessions": return [];
      case "list_tasks": return [delegatedTask];
      case "get_task_dependencies": return [];
      case "list_checkpoints": return [];
      case "list_learning_events": return [];
      case "list_skills": return [];
      case "list_models": return [];
      case "get_endpoint_active_model": return session.model_id;
      case "list_git_remotes": return [remote];
      case "list_issues": return [issue];
      case "git_status":
        return {
          branch: "codex/repo-owned-specs",
          upstream: null,
          ahead: 0,
          behind: 0,
          staged: [],
          unstaged: [],
          untracked: [],
          is_repo: true,
        };
      case "git_branches": return [];
      case "git_log": return [];
      case "get_today_cost":
      case "get_monthly_cost":
      case "get_session_cost":
        return { input_tokens: 0, output_tokens: 0, cost_usd: 0 };
      default: return null;
    }
  },
  { shouldMockEvents: true },
);

const { freshRuntime, useChatStore } = await import("../stores/chat");
const { useSettingsStore, applyTheme } = await import("../stores/settings");
const { useTasksStore } = await import("../stores/tasks");
const { WorkspacePage } = await import("../pages/Workspace/WorkspacePage");

const settings = {
  endpoints: {
    openrouter: {
      base_url: "https://openrouter.ai/api/v1",
      api_style: "openai",
      active_model: session.model_id,
    },
  },
  default_endpoint: "openrouter",
  default_model: session.model_id,
  theme: "dark",
  font_family: "inter",
  font_size: 14,
  permissions: { allow: [], ask: [], deny: [], full_access: false },
  shell: { shell: "/bin/zsh" },
  auto_create_pr: false,
} as Settings;
applyTheme(settings);
useSettingsStore.setState({ settings });
useChatStore.setState({
  sessions: [session],
  quickSessions: [],
  activeSession: session,
  draftSession: null,
  activeModel: session.model_id,
  runtime: { [sessionId]: freshRuntime() },
});
useTasksStore.setState({
  tasks: { [sessionId]: [delegatedTask] },
  running: { [sessionId]: false },
  executionLog: { [sessionId]: [] },
});

createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <div className="h-screen">
      <WorkspacePage
        sessionId={sessionId}
        onBackHome={() => {}}
        onOpenSettings={() => {}}
        onOpenSession={() => {}}
      />
    </div>
  </React.StrictMode>,
);
