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

export interface ExecutionWorkspaceView {
  objective_id: string;
  worktree_path: string;
  branch_name: string;
  base_ref: string;
  base_sha: string;
  state: "allocating" | "active" | "delivering" | "cleanup_pending" | "closed" | "incident";
  failure_code?: string | null;
  failure_detail?: string | null;
}

export function onExecutionWorkspace(
  sessionId: string,
  handler: (workspace: ExecutionWorkspaceView) => void,
): Promise<UnlistenFn> {
  if (typeof window === "undefined" || !("__TAURI_INTERNALS__" in window)) {
    return Promise.resolve(() => {});
  }
  return listen<ExecutionWorkspaceView>(`execution_workspace:${sessionId}`, (event) =>
    handler(event.payload),
  );
}

/// Progress while the app-managed Chromium is downloaded.
export interface ChromiumProgress {
  stage: "resolving" | "downloading" | "extracting" | "done";
  received_bytes?: number;
  total_bytes?: number | null;
  version?: string;
}

export interface EmbeddedBrowserEscapePayload {
  session_id: string;
}

/// Escape pressed while the native child webview owns keyboard focus.
///
/// A child webview is a separate native surface, so DOM keyboard events cannot
/// bubble into the main React document. Rust bridges that one key through this
/// event without granting the remote page access to Tauri IPC.
export function onEmbeddedBrowserEscape(
  handler: (payload: EmbeddedBrowserEscapePayload) => void,
): Promise<UnlistenFn> {
  return listen<EmbeddedBrowserEscapePayload>("embedded-browser:escape", (event) =>
    handler(event.payload),
  );
}

/// Subscribe to Chromium download progress.
///
/// Wrapped here rather than importing `listen` in the component: a component
/// that reaches for `@tauri-apps/api/event` directly blows up in any test that
/// renders it without stubbing that module, because jsdom has no Tauri runtime.
/// Every listener in this app goes through this file for that reason.
export function onChromiumProgress(
  handler: (progress: ChromiumProgress) => void,
): Promise<UnlistenFn> {
  // Outside a Tauri window there is no event bus to subscribe to, and the API
  // throws asynchronously — which surfaces as an unhandled rejection that fails
  // the whole test run while every test still reports as passing. Nothing can
  // emit this event outside the app, so a no-op is the honest answer.
  if (typeof window === "undefined" || !("__TAURI_INTERNALS__" in window)) {
    return Promise.resolve(() => {});
  }
  return listen<ChromiumProgress>("browser:chromium-progress", (event) =>
    handler(event.payload),
  );
}

// ── Types mirroring Rust StreamEvent ────────────────────────────────────────

export type StreamEvent =
  | { type: "text_delta"; content: string }
  | {
      type: "plan_updated";
      root_turn_id: string;
      revision: number;
      steps: Array<{
        id: string;
        title: string;
        kind: "analysis" | "implementation" | "verification" | "delivery" | "external_job" | "other";
        status: "pending" | "in_progress" | "completed";
        external_job_id?: string | null;
      }>;
      explanation?: string | null;
      waiting_reason?: string | null;
      next_action_owner?: "system" | "external" | "user" | null;
      change_reason?: string | null;
      created_at: number;
    }
  | {
      type: "turn_activity_updated";
      root_turn_id: string;
      revision: number;
      phase: string;
      status: string;
      recent_activity_kind: string;
      recent_activity_label: string;
      waiting_reason?: string | null;
      updated_at: number;
      terminal_reason?: string | null;
      objective_id?: string;
      objective_status?: TurnActivitySnapshot["objective_status"];
      recovery_owner?: string | null;
      next_observation_at?: number | null;
      last_progress_at?: number | null;
    }
  | { type: "tool_call_start"; id: string; name: string; args: unknown }
  | { type: "tool_call_args_delta"; index: number; chunk: string }
  | { type: "tool_call_end"; index: number }
  | {
      type: "tool_result";
      tool_call_id: string;
      content: string;
      is_error: boolean;
      status: "done" | "waiting" | "blocked" | "error" | "denied" | "cancelled";
      metadata?: Record<string, unknown> | null;
    }
  | {
      type: "permission_request";
      intent_id: string;
      tool_call_id: string;
      tool_name: string;
      args: unknown;
      expires_at?: number;
    }
  | { type: "done"; input_tokens: number; output_tokens: number }
  | {
      type: "turn_settled";
      run_instance_id: string;
      root_turn_id?: string | null;
      objective_id?: string | null;
      status:
        | "completed"
        | "cancelled"
        | "system_incident"
        | "waiting_system"
        | "waiting_user"
        | "failed_setup";
    }
  | {
      type: "context_usage";
      used_tokens: number;
      limit_tokens: number;
      max_limit_tokens?: number;
    }
  | { type: "context_compressed"; elided_count: number; tokens_freed: number }
  | {
      type: "transport_retry";
      label: string;
      attempt: number;
      max_attempts: number;
      delay_ms: number;
      reason: string;
    }
  | { type: "completion_gate_action"; kind: string; detail: string }
  | { type: "steer_applied"; message_id: string | null; content: string }
  | {
      type: "runtime_error";
      code: string;
      message: string;
      endpoint_id?: string | null;
      recoverable: boolean;
    }
  | { type: "error"; message: string };

export interface Session {
  id: string;
  title: string;
  /** Origin of the title lifecycle. Missing values are legacy rows. */
  title_source?: "placeholder" | "generated" | "fallback" | "manual" | "legacy";
  cwd: string;
  model_id: string;
  /** Endpoint owned by this session. Null on unresolved legacy rows only. */
  endpoint_id?: string | null;
  /** Per-session routing strategy. */
  model_policy?: "fixed" | "prefer" | "auto";
  created_at: number;
  updated_at: number;
  total_input_tokens: number;
  total_output_tokens: number;
  /** "project" (default) for full software-factory sessions, "quick" for an
   *  ephemeral chat launched from Quick Task, "anonymous" for a private/no-trace
   *  chat that is NEVER persisted (frontend-memory only — see sendMessageAnonymous).
   *  Optional for backward compat — old code paths default to "project". */
  kind?: "project" | "quick" | "anonymous";
  /** Per-session tool permission preset. Missing legacy rows behave as standard. */
  permission_mode?: PermissionMode;
  /** Per-session reasoning effort override; null/undefined → global default. */
  reasoning_effort?: ReasoningEffort | null;
}

export interface Message {
  id: string;
  session_id: string;
  role: "user" | "assistant" | "tool" | "system";
  content: string;
  endpoint_id?: string;
  model_id?: string;
  input_tokens?: number;
  output_tokens?: number;
  /** Serialized provider tool declarations on assistant messages. */
  tool_calls?: string | null;
  /** Completion-gate provenance: "rejected_candidate" on assistant replies
   *  the gate rejected (UI collapses them), "gate_recovery"/"gate_ready" on
   *  injected gate instructions persisted as user-role turns (UI renders
   *  them as system notices). */
  completion_state?: string | null;
  created_at: number;
}

export interface MessagePage {
  messages: Message[];
  plans?: TurnPlanSnapshot[];
  turn_states?: TurnActivitySnapshot[];
  has_more: boolean;
  next_before_rowid?: number | null;
  truncated?: boolean;
}

export interface TurnActivitySnapshot {
  root_turn_id: string;
  revision: number;
  phase: string;
  status: string;
  recent_activity_kind: string;
  recent_activity_label: string;
  waiting_reason?: string | null;
  updated_at: number;
  terminal_reason?: string | null;
  turn_settled_at?: number | null;
  stream_closed_at?: number | null;
  terminal_revision?: number | null;
  objective_id?: string;
  objective_status?: "active" | "waiting_system" | "waiting_core_input" | "waiting_authorization" | "waiting_business_decision" | "completed" | "cancelled" | "legacy_orphan";
  is_current_objective_turn?: boolean;
  recovery_owner?: string | null;
  next_observation_at?: number | null;
  last_progress_at?: number | null;
}

export interface TurnPlanSnapshot {
  root_turn_id: string;
  revision: number;
  steps: Array<{
    id: string;
    title: string;
    kind: "analysis" | "implementation" | "verification" | "delivery" | "external_job" | "other";
    status: "pending" | "in_progress" | "completed";
    external_job_id?: string | null;
  }>;
  explanation?: string | null;
  waiting_reason?: string | null;
  next_action_owner?: "system" | "external" | "user" | null;
  change_reason?: string | null;
  waiting_history?: string[];
  change_history?: string[];
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

export type ReasoningEffort = "minimal" | "low" | "medium" | "high" | "xhigh" | "max" | "ultra";

export interface CustomModel {
  id: string;
  name?: string;
  context_length?: number;
  max_context_length?: number;
  effective_context_window_percent?: number;
  default_reasoning_effort?: ReasoningEffort;
  supported_reasoning_efforts?: ReasoningEffort[];
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

export interface CodexLoginFlow {
  flow_id: string;
  authorization_url: string;
  status: "waiting" | "exchanging" | "succeeded" | "failed" | "cancelled" | "expired";
  expires_at: number;
  browser_open_error?: string | null;
  error_code?: string | null;
  error_message?: string | null;
  account?: CodexAccount | null;
}

/** Run the interactive ChatGPT OAuth login: opens the system browser, captures
 *  the localhost:1455 callback, exchanges the code, and stores tokens in the OS
 *  keychain. Resolves with the signed-in account, or rejects on error/timeout.
 *  The `app` handle is injected by Tauri, so no JS arguments are passed. */
export function codexLogin(): Promise<CodexAccount> {
  return invoke<CodexAccount>("codex_login");
}

/** Start or reuse the shared non-blocking ChatGPT authorization flow. */
export function codexLoginStart(): Promise<CodexLoginFlow> {
  return invoke<CodexLoginFlow>("codex_login_start");
}

/** Ask the OS to open the existing flow URL again. The same URL remains
 * copyable even if the system opener reports an error. */
export function codexLoginOpen(flowId: string): Promise<CodexLoginFlow> {
  return invoke<CodexLoginFlow>("codex_login_open", { flowId });
}

export function codexLoginStatus(flowId: string): Promise<CodexLoginFlow> {
  return invoke<CodexLoginFlow>("codex_login_status", { flowId });
}

export function codexLoginCancel(flowId: string): Promise<CodexLoginFlow> {
  return invoke<CodexLoginFlow>("codex_login_cancel", { flowId });
}

/** Sign out: remove the stored ChatGPT tokens from the OS keychain. */
export function codexLogout(): Promise<void> {
  return invoke<void>("codex_logout");
}

/** The currently signed-in ChatGPT account, or null if not signed in. */
export function codexAccount(): Promise<CodexAccount | null> {
  return invoke<CodexAccount | null>("codex_account");
}

export function codexModels(): Promise<CustomModel[]> {
  return invoke<CustomModel[]>("codex_models");
}

export function applyCodexModels(models: CustomModel[]): Promise<void> {
  return invoke<void>("apply_codex_models", { models });
}

export interface BrowserSession {
  session_id: string;
  task_id?: string | null;
  owner_session_id?: string | null;
  kind?: "managed" | "attached_chrome";
  updated_at_unix_secs: number;
  expired: boolean;
  status?: string | null;
  pane_url?: string | null;
  current_host?: string | null;
  page_title?: string | null;
}

export function listBrowserSessions(): Promise<BrowserSession[]> {
  return invoke<BrowserSession[]>("list_browser_sessions");
}

export function closeBrowserSession(sessionId: string): Promise<void> {
  return invoke<void>("close_browser_session", { sessionId });
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
  override_storage_mb?: number | null;
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
  override_storage_mb?: number | null;
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

export interface ModelConsistencySummary { model: string; run_id: string; total: number; passed: number; pass_rate: number; }
export interface PairwiseConsistency { model_a: string; model_b: string; jaccard: number; both_passed: number; a_only: number; b_only: number; }
export interface DivergentTask { task_name: string; per_model: Record<string, number>; spread: number; }
export interface FailureBucket { model: string; failure_class: string; count: number; }
export interface ConsistencyReport {
  dataset: string; dataset_version: string;
  models: ModelConsistencySummary[];
  pairwise: PairwiseConsistency[];
  divergent_tasks: DivergentTask[];
  failure_distribution: FailureBucket[];
  comparability_note: string;
}

export function benchmarkConsistencyReport(dataset: string, datasetVersion?: string): Promise<ConsistencyReport> {
  return invoke<ConsistencyReport>("benchmark_consistency_report", {
    request: { dataset, dataset_version: datasetVersion ?? null },
  });
}

export function importBenchmarkResults(jobPath: string): Promise<ImportedBenchmarkRun> {
  return invoke<ImportedBenchmarkRun>("import_benchmark_results", {
    request: { job_path: jobPath },
  });
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
  endpointId?: string | null,
  modelPolicy?: "fixed" | "prefer" | "auto",
): Promise<void> {
  return invoke<void>("send_message_anonymous", {
    sessionId,
    content,
    history,
    cwd,
    modelId,
    endpointId,
    modelPolicy,
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
export type PermissionMode = 'safe' | 'standard' | 'trusted';

export interface Settings {
  endpoints: Record<string, Endpoint>;
  default_endpoint: string;
  default_model: string;
  /** Routing strategy copied into new sessions; existing sessions are unchanged. */
  default_model_policy?: "fixed" | "prefer" | "auto";
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
  /** Opt-in for sending a bounded, redacted post-mortem summary to the
   * configured model after a session. Local deterministic mining is unaffected. */
  remote_postmortem_enabled?: boolean;
  theme: Theme;
  font_family: string;
  mono_font_family: string;
  font_size: number;
  /** Default reasoning effort for reasoning-capable models (chatgpt/codex
   *  Responses path). Optional for backward compat — missing → "medium". */
  reasoning_effort?: ReasoningEffort;
  /** True once the user has completed (or skipped) first-run onboarding.
   *  Optional for backward compat — missing/false triggers the overlay. */
  onboarded?: boolean;
  /** Max concurrent subagents for parallel tasks (backend clamps to 1..=8).
   *  Optional for backward compat — missing → 3. */
  max_parallel_tasks?: number;
  /** Disk isolation for parallel subagents. Optional for backward compat —
   *  missing → "shared". "worktree" runs each task in its own git worktree
   *  and merges verified diffs back; non-git cwds fall back to shared. */
  subagent_isolation?: 'shared' | 'worktree';
  /** How far the agent auto-delivers code changes. The user can lower this boundary.
   *  Optional for backward compat — missing → "through_release". */
  delivery_ceiling?: 'off' | 'pr_only' | 'through_ci_green' | 'through_merge' | 'through_release';
  /** Whether the user explicitly chose a delivery ceiling; legacy missing/old defaults are migrated. */
  delivery_ceiling_explicit?: boolean;
  /** One-way IM notifications for task terminals / turn errors / permission waits. Empty URL = off. */
  im_webhook_url?: string;
  im_webhook_format?: 'wecom' | 'feishu' | 'generic';
  /** Shell isolation for the bash tool: docker wraps every command in a disposable container mounting only the project dir. */
  sandbox_mode?: 'off' | 'docker';
  sandbox_image?: string;
  /** Merge strategy for delivery at ThroughMerge+. Missing → "squash". */
  delivery_merge_method?: 'squash' | 'merge' | 'rebase';
  /** Extra repo-relative path prefixes excluded from delivery commits. */
  delivery_exclude_globs?: string[];
  /** Max seconds delivery polls CI before reporting it still pending. Missing → 1800. */
  delivery_ci_timeout_secs?: number;
  usage_budget?: {
    daily_token_limit: number;
    monthly_token_limit: number;
    alert_thresholds: number[];
    alerts_enabled: boolean;
  };
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

export interface TaskFailureAttribution {
  kind:
    | "model-provider"
    | "permission"
    | "shell-runtime"
    | "test-failure"
    | "verification"
    | "cancelled"
    | "unknown";
  label: string;
  summary: string;
  next_action: string;
  repairable: boolean;
  source: string;
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
  /** Derived failure attribution; not persisted in SQLite. */
  failure_attribution?: TaskFailureAttribution | null;
  /** The spec this task was decomposed from (set by a spec's 开始实现); null for
   *  ad-hoc Workspace tasks. Surfaces the spec→task link in the task tree. */
  spec_req_id?: string | null;
  spec_title?: string | null;
  attempts?: TaskAttempt[];
}

export interface TaskAttempt {
  id: string;
  task_id: string;
  attempt_index: number;
  sub_session_id: string | null;
  status: string;
  failure_code: string | null;
  started_at: string;
  completed_at: string | null;
  error: string | null;
  result: string | null;
  verification_results: string | null;
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

export interface GithubCliCredentialStatus {
  installed: boolean;
  authenticated: boolean;
}

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
