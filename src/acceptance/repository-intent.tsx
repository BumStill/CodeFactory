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
const sidebarSessions: Session[] = Array.from({ length: 12 }, (_, index) => ({
  ...session,
  id: index === 0 ? sessionId : `repository-session-${index}`,
  title: index === 0 ? session.title : `会话 ${index + 1}`,
  cwd: `${cwd}/project-${index + 1}`,
  updated_at: Date.now() - index * 60 * 60 * 1000,
}));

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
const pendingAfterFailure: TaskRun = {
  ...delegatedTask,
  id: "pending-after-provider-failures",
  title: "等待模型配置修复后的后续任务",
  status: "pending",
  started_at: null,
  completed_at: null,
  error: null,
  failure_attribution: null,
};
const providerFailureTitles = [
  "独立 UX 审查",
  "浏览器视口 QA",
  "任务抽屉独立审查",
  "侧栏与命令层级审查",
  "审查 GitHub 交付状态数据链",
  "审查状态栏信息架构与窄屏",
];
const providerFailures: TaskRun[] = providerFailureTitles.map((title, index) => ({
  ...delegatedTask,
  id: `provider-failure-${index + 1}`,
  title,
  status: "failed",
  started_at: "2026-07-23T00:00:00Z",
  completed_at: "2026-07-23T00:01:00Z",
  error: "API key not found for key_ref 'codefactory.endpoint.chatgpt'",
  failure_attribution: {
    kind: "model-provider",
    label: "模型/Provider",
    summary: "API key not found for key_ref 'codefactory.endpoint.chatgpt'",
    next_action: "打开模型设置，修复 endpoint、API key、余额或模型 route 后再重试。",
    repairable: false,
    source: "error",
  },
}));
const fixtureTasks = [delegatedTask, pendingAfterFailure, ...providerFailures];

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
  (command, args) => {
    switch (command) {
      case "get_settings": return settings;
      case "list_sessions": return sidebarSessions;
      case "list_quick_sessions": return [];
      case "list_tasks": return fixtureTasks;
      case "retry_tasks":
        (window as typeof window & { __lastRetry?: unknown }).__lastRetry = args;
        return providerFailures.length;
      case "start_implementation":
        (window as typeof window & { __implementationStarted?: boolean }).__implementationStarted = true;
        return null;
      case "get_task_dependencies": return [];
      case "list_checkpoints": return [];
      case "list_learning_events": return [];
      case "list_skills": return [];
      case "list_models": return [];
      case "get_endpoint_active_model": return session.model_id;
      case "list_git_remotes": return [remote];
      case "list_issues": return [issue];
      case "workspace_delivery_status":
        return {
          remote_available: true,
          pr: {
            number: 175,
            title: "Improve workspace status",
            state: "merged",
            draft: false,
            head_branch: "feat/workspace-ui",
            base_branch: "main",
            head_sha: "3373a69963b1ef2a191f06dd1397be3ddf043435",
            merge_commit_sha: "2c2af7728065520a73d93ef16376d738381eb42f",
            url: "https://github.com/BumStill/CodeFactory/pull/175",
          },
          ci_status: "success",
          release: {
            tag: "v1.63.0",
            url: "https://github.com/BumStill/CodeFactory/releases/tag/v1.63.0",
            published_at: "2026-07-23T00:00:00Z",
          },
          error: null,
        };
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
const { SettingsPage } = await import("../pages/Settings/SettingsPage");

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
  sessions: sidebarSessions,
  activeSession: session,
  draftSession: null,
  activeModel: session.model_id,
  runtime: {
    [sessionId]: {
      ...freshRuntime(),
      messages: [{
        id: "tool-density-message",
        role: "assistant",
        content: "已完成界面整理。",
        createdAt: Date.now(),
        segments: [
          { kind: "tool", toolCallId: "read-1" },
          { kind: "tool", toolCallId: "edit-1" },
          { kind: "tool", toolCallId: "test-1" },
          { kind: "tool", toolCallId: "failed-1" },
          { kind: "text", text: "已完成界面整理。" },
        ],
        toolCalls: [
          { id: "read-1", name: "read_file", args: JSON.stringify({ path: "src/App.tsx" }), status: "done", result: "ok" },
          { id: "edit-1", name: "edit_file", args: JSON.stringify({ path: "src/App.tsx", old_string: "a", new_string: "b" }), status: "done", result: "ok" },
          { id: "test-1", name: "bash", args: JSON.stringify({ command: "npm test" }), status: "done", result: "ok" },
          { id: "failed-1", name: "bash", args: JSON.stringify({ command: "npm run check" }), status: "error", isError: true, result: "check failed" },
        ],
      }],
    },
  },
});
useTasksStore.setState({
  tasks: { [sessionId]: fixtureTasks },
  running: { [sessionId]: false },
  executionLog: { [sessionId]: [] },
});

function AcceptanceApp() {
  const [settingsTab, setSettingsTab] = React.useState<"endpoints" | "permissions" | null>(null);
  return (
    <div className="h-screen">
      {settingsTab ? (
        <SettingsPage onBack={() => setSettingsTab(null)} initialTab={settingsTab} />
      ) : (
        <WorkspacePage
          sessionId={sessionId}
          onNewConversation={() => {}}
          onOpenSettings={(tab) => {
            const target = tab === "permissions" ? "permissions" : "endpoints";
            (window as typeof window & { __settingsTab?: string }).__settingsTab = target;
            setSettingsTab(target);
          }}
          onOpenSession={() => {}}
        />
      )}
    </div>
  );
}

createRoot(document.getElementById("root")!).render(
  <React.StrictMode><AcceptanceApp /></React.StrictMode>,
);
