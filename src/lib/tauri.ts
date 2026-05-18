// SPDX-License-Identifier: Apache-2.0
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

export { invoke };

export function onStream(
  sessionId: string,
  handler: (event: StreamEvent) => void
): Promise<UnlistenFn> {
  return listen<StreamEvent>(`stream:${sessionId}`, (e) => handler(e.payload));
}

export function onSessionUpdated(
  sessionId: string,
  handler: (session: Session) => void
): Promise<UnlistenFn> {
  return listen<Session>(`session_updated:${sessionId}`, (e) => handler(e.payload));
}

// ── Types mirroring Rust StreamEvent ────────────────────────────────────────

export type StreamEvent =
  | { type: "text_delta"; content: string }
  | { type: "tool_call_start"; id: string; name: string; args: unknown }
  | { type: "tool_call_args_delta"; index: number; chunk: string }
  | { type: "tool_call_end"; index: number }
  | { type: "tool_result"; tool_call_id: string; content: string; is_error: boolean }
  | { type: "permission_request"; tool_call_id: string; tool_name: string; args: unknown }
  | { type: "done"; input_tokens: number; output_tokens: number }
  | { type: "error"; message: string };

export interface Session {
  id: string;
  title: string;
  cwd: string;
  model_id: string;
  created_at: number;
  updated_at: number;
  total_input_tokens: number;
  total_output_tokens: number;
}

export interface Message {
  id: string;
  session_id: string;
  role: "user" | "assistant" | "tool" | "system";
  content: string;
  model_id?: string;
  input_tokens?: number;
  output_tokens?: number;
  created_at: number;
}

export interface ModelInfo {
  id: string;
  name: string;
  context_length: number;
  pricing?: { prompt: string; completion: string };
}

export type ApiStyle = "openai" | "anthropic";

export interface Endpoint {
  base_url: string;
  key_ref?: string;
  api_style: ApiStyle;
}

export interface Settings {
  endpoints: Record<string, Endpoint>;
  default_endpoint: string;
  default_model: string;
  permissions: {
    allow: string[];
    ask: string[];
    deny: string[];
    full_access: boolean;
  };
  shell: {
    shell: string;
  };
  auto_create_pr: boolean;
}

// ── Git ─────────────────────────────────────────────────────────────────────

export interface FileChange {
  path: string;
  status: "modified" | "added" | "deleted" | "renamed" | "typechange";
}

export interface GitStatus {
  branch: string;
  upstream: string | null;
  ahead: number;
  behind: number;
  staged: FileChange[];
  unstaged: FileChange[];
  untracked: string[];
  is_repo: boolean;
}

export interface GitCommit {
  hash: string;
  short_hash: string;
  author: string;
  email: string;
  timestamp: number;
  message: string;
  message_body: string;
}

export interface GitBranch {
  name: string;
  is_current: boolean;
  is_remote: boolean;
  upstream: string | null;
}

// ── Tasks (Phase 2 + 3) ─────────────────────────────────────────────────────

export type TaskStatus =
  | "pending"
  | "running"
  | "completed"
  | "failed"
  | "cancelled";

export interface VerificationResult {
  check: string;
  passed: boolean;
  output: string;
  duration_ms: number;
}

export interface TaskRun {
  id: string;
  session_id: string;
  title: string;
  description: string;
  status: TaskStatus;
  cwd: string;
  parent_task_id: string | null;
  sub_session_id: string | null;
  created_at: string;
  started_at: string | null;
  completed_at: string | null;
  result: string | null;
  error: string | null;
  attempt_count: number;
  /** JSON-encoded Vec<VerificationResult>; null when not yet run. */
  verification_results: string | null;
}

export interface TaskInput {
  tmp_id: string;
  title: string;
  description: string;
  cwd: string;
}

export interface TaskDep {
  task_tmp_id: string;
  depends_on_tmp_id: string;
}

export interface TaskEventPayload {
  task_id: string;
  title?: string;
  message?: string;
  result?: string;
  error?: string;
}

export type TaskEventKind =
  | "task_started"
  | "task_progress"
  | "task_completed"
  | "task_failed"
  | "task_retry"
  | "task_verification";

// ── Evidence Packs (Phase 6) ────────────────────────────────────────────────

export interface EvidencePackMeta {
  spec_req_id: string;
  spec_title: string;
  task_run_ids: string[];
  session_id: string;
  created_at: string;
  completed_at: string;
  status: "passed" | "failed" | "partial";
  total_tasks: number;
  completed_tasks: number;
  failed_tasks: number;
  total_tool_calls: number;
  files_changed: number;
  verification_passed: boolean;
  total_tokens: number;
  duration_minutes: number;
  path: string;
}

export interface EvidencePackReadyPayload {
  spec_req_id: string;
  spec_title: string;
  path: string;
}

// ── Git Remote (Phase 8) ─────────────────────────────────────────────────────

export type GitProvider = "github" | "gitlab";

export interface GitRemoteConfig {
  id: string;
  name: string;
  provider: GitProvider;
  base_url: string;
  token: string;
  default_repo: string | null;
}

export interface RemoteIssue {
  id: number;
  number: number;
  title: string;
  body: string;
  state: string;
  labels: string[];
  created_at: string;
  updated_at: string;
  url: string;
  author: string;
}

export interface RemotePR {
  id: number;
  number: number;
  title: string;
  body: string;
  state: string;
  base_branch: string;
  head_branch: string;
  created_at: string;
  url: string;
  draft: boolean;
}

export interface RemoteRepo {
  full_name: string;
  description: string;
  default_branch: string;
  url: string;
  private: boolean;
  stars: number;
}
