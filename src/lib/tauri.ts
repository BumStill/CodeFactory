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
  | { type: "context_usage"; used_tokens: number; limit_tokens: number }
  | { type: "context_compressed"; elided_count: number; tokens_freed: number }
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
  /** "project" (default) for full software-factory sessions, "quick" for an
   *  ephemeral chat launched from Quick Task, "anonymous" for a private/no-trace
   *  chat that is NEVER persisted (frontend-memory only — see sendMessageAnonymous).
   *  Optional for backward compat — old code paths default to "project". */
  kind?: "project" | "quick" | "anonymous";
  /** Per-session reasoning effort override; null/undefined → global default. */
  reasoning_effort?: ReasoningEffort | null;
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
  /** True when this entry came from Endpoint.custom_models. */
  is_custom?: boolean;
}

export type ApiStyle = "openai" | "anthropic" | "chatgpt";

export type ReasoningEffort = "minimal" | "low" | "medium" | "high";

export interface CustomModel {
  id: string;
  name?: string;
  context_length?: number;
}

export interface Endpoint {
  base_url: string;
  key_ref?: string;
  api_style: ApiStyle;
  custom_models?: CustomModel[];
  /** Per-endpoint remembered model selection — set when the user picks one
   * in the ModelPicker or Settings. Auto-applies on endpoint switch so the
   * previous endpoint's id (often vendor-prefixed) doesn't carry over. */
  active_model?: string;
}

// ── ChatGPT (Codex) OAuth login ─────────────────────────────────────────────

/** Public account info from a ChatGPT sign-in — never includes raw tokens. */
export interface CodexAccount {
  email?: string | null;
  plan?: string | null;
  account_id?: string | null;
}

/** Run the interactive ChatGPT OAuth login: opens the system browser, captures
 *  the localhost:1455 callback, exchanges the code, and stores tokens in the OS
 *  keychain. Resolves with the signed-in account, or rejects on error/timeout.
 *  The `app` handle is injected by Tauri, so no JS arguments are passed. */
export function codexLogin(): Promise<CodexAccount> {
  return invoke<CodexAccount>("codex_login");
}

/** Sign out: remove the stored ChatGPT tokens from the OS keychain. */
export function codexLogout(): Promise<void> {
  return invoke<void>("codex_logout");
}

/** The currently signed-in ChatGPT account, or null if not signed in. */
export function codexAccount(): Promise<CodexAccount | null> {
  return invoke<CodexAccount | null>("codex_account");
}

// ── Benchmarks ─────────────────────────────────────────────────────────────

export interface BenchmarkProfile {
  id: string;
  dataset: string;
  harness: string;
  official_url: string;
  leaderboard_url: string;
  comparable_constraints: string[];
  default_smoke_task_limit: number;
}

export type BenchmarkProbeStatus = "ok" | "missing" | "warning";

export interface BenchmarkProbeItem {
  id: string;
  label: string;
  status: BenchmarkProbeStatus;
  detail: string;
}

export interface BenchmarkEnvironmentProbe {
  generated_at: string;
  profile: BenchmarkProfile;
  ready: boolean;
  blockers: string[];
  items: BenchmarkProbeItem[];
  command_preview: string;
}

export interface BenchmarkEnvVarPreview {
  name: string;
  value: string;
  secret: boolean;
}

export interface BenchmarkProviderBridgeRequest {
  profile_id: string;
  endpoint_name?: string | null;
  model?: string | null;
  task_limit?: number | null;
  task_names?: string[] | null;
  concurrency?: number | null;
  /** Deprecated compatibility alias; Harbor `-n` means concurrency. */
  trial_count?: number | null;
  job_root?: string | null;
  job_name?: string | null;
  adapter_root?: string | null;
}

export interface BenchmarkProviderBridgePreview {
  generated_at: string;
  profile: BenchmarkProfile;
  endpoint_name: string;
  base_url: string;
  api_style: string;
  model: string;
  key_ref: string;
  agent_import_path: string;
  task_limit: number;
  task_names: string[];
  concurrency: number;
  trial_count: number;
  job_root: string;
  job_name: string;
  job_path: string;
  adapter_root: string;
  env_preview: BenchmarkEnvVarPreview[];
  command_preview: string;
  authorization_phrase: string;
  ready: boolean;
  blockers: string[];
}

export interface BenchmarkRunRecord {
  id: string;
  benchmark_id: string;
  dataset: string;
  dataset_version?: string | null;
  agent_name: string;
  agent_version?: string | null;
  model?: string | null;
  codefactory_version?: string | null;
  codefactory_git_sha?: string | null;
  policy_preset: string;
  harbor_version?: string | null;
  command: string;
  job_path: string;
  status: string;
  started_at: string;
  finished_at?: string | null;
  comparable: boolean;
  comparable_reason?: string | null;
  missing_files: string[];
}

export interface BenchmarkTrialRecord {
  id: string;
  run_id: string;
  task_name: string;
  category?: string | null;
  difficulty?: string | null;
  reward: number;
  duration_ms?: number | null;
  error_kind?: string | null;
  failure_class?: string | null;
  failure_reason?: string | null;
  trajectory_path?: string | null;
  verifier_stdout_path?: string | null;
  verifier_stderr_path?: string | null;
}

export interface ImportedBenchmarkRun {
  run: BenchmarkRunRecord;
  trials: BenchmarkTrialRecord[];
}

export interface BenchmarkProviderRunResult {
  preview: BenchmarkProviderBridgePreview;
  status: string;
  failure_kind?: string | null;
  blocker?: string | null;
  exit_code?: number | null;
  stdout: string;
  stderr: string;
  imported?: ImportedBenchmarkRun | null;
}

export function listBenchmarkProfiles(): Promise<BenchmarkProfile[]> {
  return invoke<BenchmarkProfile[]>("list_benchmark_profiles");
}

export function probeBenchmarkEnvironment(profileId: string): Promise<BenchmarkEnvironmentProbe> {
  return invoke<BenchmarkEnvironmentProbe>("probe_benchmark_environment", { profileId });
}

export function previewBenchmarkProviderBridge(
  request: BenchmarkProviderBridgeRequest,
): Promise<BenchmarkProviderBridgePreview> {
  return invoke<BenchmarkProviderBridgePreview>("preview_benchmark_provider_bridge", { request });
}

export function startBenchmarkProviderRun(
  bridge: BenchmarkProviderBridgeRequest,
  authorizationPhrase: string,
): Promise<BenchmarkProviderRunResult> {
  return invoke<BenchmarkProviderRunResult>("start_benchmark_provider_run", {
    request: { bridge, authorization_phrase: authorizationPhrase },
  });
}

export function importBenchmarkResults(jobPath: string): Promise<ImportedBenchmarkRun> {
  return invoke<ImportedBenchmarkRun>("import_benchmark_results", {
    request: { job_path: jobPath },
  });
}

// ── Quick Task sessions (multi-session) ─────────────────────────────────────

/** Resume the most-recent Quick Task session, creating the first one on
 *  demand. Backs Home's "快速任务" card (continue-latest semantics). */
export function getOrCreateQuickSession(modelId: string): Promise<Session> {
  return invoke<Session>("get_or_create_quick_session", { modelId });
}

/** Always create a *fresh* Quick Task session with its own scratch dir.
 *  Backs the "+ 新建快速任务" action in the switcher. */
export function createQuickSession(modelId: string): Promise<Session> {
  return invoke<Session>("create_quick_session", { modelId });
}

/** List Quick Task sessions, most-recent first (for the Home switcher). */
export function listQuickSessions(): Promise<Session[]> {
  return invoke<Session[]>("list_quick_sessions");
}

/** One prior turn of an anonymous conversation (role + text). */
export interface AnonTurn {
  role: "user" | "assistant";
  content: string;
}

/** Send a message in an ANONYMOUS / ephemeral session. Nothing is persisted
 *  server-side: no messages, no cost, no checkpoints, no session row. The
 *  frontend owns the whole history (`history`), which is replayed to the model
 *  each turn. `sessionId` is a client-generated id used only to route stream
 *  events; `cwd` may be "" to let the backend use the default scratch dir. */
export function sendMessageAnonymous(
  sessionId: string,
  content: string,
  history: AnonTurn[],
  cwd: string,
  modelId: string,
): Promise<void> {
  return invoke<void>("send_message_anonymous", {
    sessionId,
    content,
    history,
    cwd,
    modelId,
  });
}

// ── Checkpoints (git-backed rollback) ──────────────────────────────────────

export interface CheckpointInfo {
  id: string;
  session_id: string;
  message_id: string | null;
  cwd: string;
  git_sha: string;
  label: string;
  created_at: string;
  reverted: boolean;
}

export interface CheckpointFileChange {
  path: string;
  status: "added" | "modified" | "deleted" | "renamed" | "typechange";
}

// ── Project memory (.codefactory/memory.md) ────────────────────────────────

export interface ProjectMemory {
  path: string;
  content: string;
  exists: boolean;
}

export type Theme = 'dark' | 'light' | 'system';

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
  theme: Theme;
  font_family: string;
  font_size: number;
  /** Default reasoning effort for reasoning-capable models (chatgpt/codex
   *  Responses path). Optional for backward compat — missing → "medium". */
  reasoning_effort?: ReasoningEffort;
  /** True once the user has completed (or skipped) first-run onboarding.
   *  Optional for backward compat — missing/false triggers the overlay. */
  onboarded?: boolean;
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

export interface TaskKnowledgeLibraryContext {
  id: string;
  name: string;
  root_path: string;
  scan_status: string;
  last_scan_at: string | null;
}

export interface TaskConnectorContext {
  knowledge_libraries: TaskKnowledgeLibraryContext[];
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
  /** JSON-encoded TaskConnectorContext; null when no connector scope is attached. */
  task_context_json: string | null;
  /** The spec this task was decomposed from (set by a spec's 开始实现); null for
   *  ad-hoc Workspace tasks. Surfaces the spec→task link in the task tree. */
  spec_req_id?: string | null;
  spec_title?: string | null;
}

export interface TaskInput {
  tmp_id: string;
  title: string;
  description: string;
  cwd: string;
  /** Concrete verifiable conditions for "done". One bullet each.
   *  The autonomous agent loop reads these and MUST verify each
   *  before reporting completion. Empty list is allowed but the
   *  decompose commands always populate it. */
  acceptance_criteria?: string[];
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

// ── Knowledge libraries ────────────────────────────────────────────────────

export interface KnowledgeLibrary {
  id: string;
  name: string;
  root_path: string;
  enabled: boolean;
  created_at: string;
  last_scan_at: string | null;
  scan_status: string;
}

export interface KnowledgeScanSummary {
  library_id: string;
  scanned_files: number;
  indexed_documents: number;
  failed_documents: number;
  chunks_indexed: number;
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
  token_ref?: string;
  default_repo: string | null;
  has_token: boolean;
}

export interface AddGitRemoteRequest {
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
