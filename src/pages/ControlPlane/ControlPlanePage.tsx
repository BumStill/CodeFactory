// SPDX-License-Identifier: Apache-2.0
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { ReactNode } from "react";
import {
  AlertTriangle,
  ChevronLeft,
  CircleCheck,
  CircleDashed,
  GitBranch,
  RefreshCcw,
  ShieldCheck,
} from "lucide-react";
import { invoke } from "../../lib/tauri";
import { useChatStore } from "../../stores/chat";

type Status = "ok" | "missing" | "warning";

interface ControlPlaneItem {
  id: string;
  label: string;
  status: Status;
  path?: string | null;
  detail: string;
}

interface MemoryProposalSummary {
  pending: number;
  accepted: number;
  rejected: number;
  preference_pending: number;
  latest_pending: string[];
}

interface CapabilitySummary {
  id: string;
  label: string;
  total: number;
  enabled: number;
  status: Status;
  detail: string;
}

type GitProbeStatus = "ok" | "partial" | "not_repository" | "unavailable" | "not_checked";

interface GitProbeSummary {
  status: GitProbeStatus;
  timeout_ms: number;
  timed_out: string[];
  failed: string[];
}

interface DeliverySummary {
  git_branch?: string | null;
  is_dirty: boolean | null;
  dirty_count: number | null;
  sync_gate_present: boolean;
  sync_gate_configured: boolean | null;
  release_workflow_present: boolean;
  auto_release_present: boolean;
  latest_release_tag?: string | null;
  git_probe?: GitProbeSummary;
}

interface ControlPlaneRisk {
  id: string;
  severity: string;
  message: string;
}

interface ControlPlaneSnapshot {
  generated_at: string;
  cwd?: string | null;
  authority: ControlPlaneItem[];
  memory: MemoryProposalSummary;
  capabilities: CapabilitySummary[];
  delivery: DeliverySummary;
  risks: ControlPlaneRisk[];
}

type ObjectiveHealthAvailability = "available" | "unavailable";

interface ObjectiveHealthMetrics {
  open: number;
  system_owned: number;
  typed_user_attention: number;
  invalid_user_attention_requests: number;
  technical_user_handoff_violations: number;
  technical_user_handoff_violations_24h: number;
  avoidable_user_reprompts_24h: number;
  overdue_ownerless_remediations: number;
  stalled_system_owned_objectives: number;
  unavailable_domain_adapter_objectives: number;
  invalid_completions: number;
  invalid_completions_24h: number;
  invalid_terminal_convergences: number;
  duplicate_committed_side_effect_receipts: number;
  duplicate_committed_side_effect_receipts_24h: number;
  requested_ceiling_downgrades_24h: number;
  recovery_decisions: number;
  recovered_objectives: number;
  recovery_latency_p50_ms: number | null;
  recovery_latency_p95_ms: number | null;
  recovery_decisions_24h: number;
  recovered_objectives_24h: number;
  recovery_latency_p50_ms_24h: number | null;
  recovery_latency_p95_ms_24h: number | null;
}

interface ObjectiveHealthSnapshot {
  generated_at_ms: number;
  window_start_ms: number;
  build_git_sha: string | null;
  build_observation_started_at_ms?: number | null;
  production_window_covered?: boolean;
  availability: ObjectiveHealthAvailability;
  unavailable_reason: string | null;
  metrics: ObjectiveHealthMetrics | null;
}

interface ControlPlanePageProps {
  onBack: () => void;
}

const CONTROL_PLANE_REQUEST_TIMEOUT_MS = 8_000;
const CONTROL_PLANE_RECOVERY_DELAY_MS = 3_000;

async function requestControlPlaneSnapshot(cwd: string | null): Promise<ControlPlaneSnapshot> {
  let timeoutId: ReturnType<typeof setTimeout> | undefined;
  const watchdog = new Promise<never>((_, reject) => {
    timeoutId = setTimeout(
      () => reject(new Error("控制面请求超过 8 秒；观测状态已保留，系统将在 3 秒后自动重新观测。")),
      CONTROL_PLANE_REQUEST_TIMEOUT_MS,
    );
  });

  try {
    return await Promise.race([
      invoke<ControlPlaneSnapshot>("get_control_plane_snapshot", { cwd }),
      watchdog,
    ]);
  } finally {
    if (timeoutId !== undefined) clearTimeout(timeoutId);
  }
}

async function requestObjectiveHealthSnapshot(): Promise<ObjectiveHealthSnapshot> {
  let timeoutId: ReturnType<typeof setTimeout> | undefined;
  const watchdog = new Promise<never>((_, reject) => {
    timeoutId = setTimeout(
      () => reject(new Error("Objective health observation exceeded 8 seconds")),
      CONTROL_PLANE_REQUEST_TIMEOUT_MS,
    );
  });

  try {
    const next = await Promise.race([
      invoke<ObjectiveHealthSnapshot>("get_objective_health"),
      watchdog,
    ]);
    if (!next || (next.availability !== "available" && next.availability !== "unavailable")) {
      throw new Error("Objective health command returned an invalid snapshot");
    }
    return next;
  } catch (error) {
    const now = Date.now();
    return {
      generated_at_ms: now,
      window_start_ms: now - 86_400_000,
      build_git_sha: null,
      availability: "unavailable",
      unavailable_reason: `Objective health observation unavailable: ${
        error instanceof Error ? error.message : String(error)
      }`,
      metrics: null,
    };
  } finally {
    if (timeoutId !== undefined) clearTimeout(timeoutId);
  }
}

function StatusBadge({ status }: { status: Status }) {
  const label = status === "ok" ? "OK" : status === "missing" ? "Missing" : "Warning";
  const tone =
    status === "ok"
      ? "border-emerald-500/30 bg-emerald-500/10 text-emerald-800 dark:text-emerald-300"
      : status === "missing"
      ? "border-red-500/30 bg-red-500/10 text-red-800 dark:text-red-300"
      : "border-amber-500/30 bg-amber-500/10 text-amber-800 dark:text-amber-300";
  return (
    <span className={`inline-flex items-center rounded border px-1.5 py-0.5 text-caption font-medium ${tone}`}>
      {label}
    </span>
  );
}

function Section({
  title,
  children,
}: {
  title: string;
  children: ReactNode;
}) {
  return (
    <section className="border-t border-border py-4">
      <h2 className="mb-3 text-label font-semibold uppercase tracking-wider text-gray-500">{title}</h2>
      {children}
    </section>
  );
}

function AuthorityGrid({ items }: { items: ControlPlaneItem[] }) {
  return (
    <div className="grid grid-cols-1 gap-2 md:grid-cols-2">
      {items.map((item) => (
        <div key={item.id} className="rounded border border-border bg-surface-1 p-3">
          <div className="mb-1 flex items-center justify-between gap-2">
            <div className="min-w-0 text-body font-medium text-gray-200">{item.label}</div>
            <StatusBadge status={item.status} />
          </div>
          <p className="text-label leading-relaxed text-gray-500">{item.detail}</p>
          {item.path && (
            <p className="mt-2 truncate font-mono text-caption text-gray-700" title={item.path}>
              {item.path}
            </p>
          )}
        </div>
      ))}
    </div>
  );
}

function CapabilityGrid({ items }: { items: CapabilitySummary[] }) {
  return (
    <div className="grid grid-cols-1 gap-2 md:grid-cols-5">
      {items.map((item) => (
        <div key={item.id} className="rounded border border-border bg-surface-1 p-3">
          <div className="mb-2 flex items-center justify-between gap-2">
            <div className="truncate text-label font-medium text-gray-300">{item.label}</div>
            <StatusBadge status={item.status} />
          </div>
          <div className="text-heading font-semibold text-gray-100">
            {item.enabled}
            <span className="text-label font-normal text-gray-600"> / {item.total}</span>
          </div>
          <p className="mt-2 text-caption leading-relaxed text-gray-600">{item.detail}</p>
        </div>
      ))}
    </div>
  );
}

function gitContextLabel(delivery: DeliverySummary): string {
  if (delivery.git_branch) return delivery.git_branch;
  switch (delivery.git_probe?.status) {
    case "partial":
      return "Git 状态部分可用";
    case "unavailable":
      return "Git unavailable";
    case "not_checked":
      return "Git not checked";
    case "not_repository":
      return "not a git repo";
    case "ok":
      return "branch unavailable";
    default:
      return "not a git repo";
  }
}

function gitObservationLabel(probe?: GitProbeSummary): string {
  if (!probe) return "legacy";
  if (probe.status === "ok") return "complete";
  if (probe.status === "not_repository") return "not applicable";
  if (probe.status === "unavailable") return "Git unavailable";
  if (probe.status === "not_checked") return "not checked";

  const reasons = [];
  if (probe.timed_out.length > 0) reasons.push(`${probe.timed_out.join(", ")} timed out`);
  if (probe.failed.length > 0) reasons.push(`${probe.failed.join(", ")} failed`);
  return reasons.length > 0 ? `partial · ${reasons.join(" · ")}` : "partial";
}

function unknownGitField(delivery: DeliverySummary, probeName: string): boolean {
  const probe = delivery.git_probe;
  if (!probe) return false;
  return (
    probe.status === "unavailable" ||
    probe.status === "not_checked" ||
    (probe.status === "partial" &&
      (probe.timed_out.includes("repository") || probe.failed.includes("repository"))) ||
    probe.timed_out.includes(probeName) ||
    probe.failed.includes(probeName)
  );
}

function DeliveryPanel({ delivery }: { delivery: DeliverySummary }) {
  const gitNotApplicable = delivery.git_probe?.status === "not_repository";
  const dirtyState = gitNotApplicable
    ? "not applicable"
    : unknownGitField(delivery, "status") || delivery.is_dirty === null
      ? "unknown"
      : delivery.is_dirty === true
        ? `${delivery.dirty_count ?? "?"} item(s)`
        : "clean";
  const syncHookState = gitNotApplicable
    ? "not applicable"
    : !delivery.sync_gate_present
      ? "missing"
      : unknownGitField(delivery, "hook") || delivery.sync_gate_configured === null
        ? "unknown"
        : delivery.sync_gate_configured
          ? "configured"
          : "not configured";
  const latestTag = delivery.latest_release_tag
    ? delivery.latest_release_tag
    : delivery.git_probe?.status === "not_repository"
      ? "not applicable"
      : unknownGitField(delivery, "tag")
        ? "unknown"
        : "none";
  const rows = [
    ["Git observation", gitObservationLabel(delivery.git_probe)],
    ["Branch", gitContextLabel(delivery)],
    ["Dirty tree", dirtyState],
    ["Sync gate", delivery.sync_gate_present ? "present" : "missing"],
    ["Sync hook config", syncHookState],
    ["Release workflow", delivery.release_workflow_present ? "present" : "missing"],
    ["Auto Release", delivery.auto_release_present ? "present" : "missing"],
    ["Latest tag", latestTag],
  ];
  return (
    <div className="grid grid-cols-1 gap-2 md:grid-cols-3">
      {rows.map(([label, value]) => (
        <div key={label} className="rounded border border-border bg-surface-1 p-3">
          <div className="text-caption uppercase tracking-wider text-gray-600">{label}</div>
          <div className="mt-1 truncate text-body font-medium text-gray-200">{value}</div>
        </div>
      ))}
    </div>
  );
}

function MemoryPanel({ memory }: { memory: MemoryProposalSummary }) {
  const total = memory.pending + memory.accepted + memory.rejected;
  return (
    <div className="grid grid-cols-1 gap-3 md:grid-cols-[280px_1fr]">
      <div className="rounded border border-border bg-surface-1 p-3">
        <div className="grid grid-cols-2 gap-2">
          <Metric label="Pending" value={memory.pending} />
          <Metric label="Accepted" value={memory.accepted} />
          <Metric label="Rejected" value={memory.rejected} />
          <Metric label="Preference" value={memory.preference_pending} />
        </div>
        <p className="mt-3 text-caption text-gray-600">{total} lifecycle item(s) in this project scope.</p>
      </div>
      <div className="rounded border border-border bg-surface-1 p-3">
        <div className="mb-2 text-label font-medium text-gray-400">Latest pending proposals</div>
        {memory.latest_pending.length === 0 ? (
          <p className="text-label text-gray-600">No pending memory proposals.</p>
        ) : (
          <ul className="space-y-2">
            {memory.latest_pending.map((item, index) => (
              <li key={`${index}-${item}`} className="line-clamp-2 text-label leading-relaxed text-gray-400">
                {item}
              </li>
            ))}
          </ul>
        )}
      </div>
    </div>
  );
}

function Metric({ label, value }: { label: string; value: number }) {
  return (
    <div>
      <div className="text-caption uppercase tracking-wider text-gray-600">{label}</div>
      <div className="text-heading font-semibold text-gray-100">{value}</div>
    </div>
  );
}

function RiskList({ risks }: { risks: ControlPlaneRisk[] }) {
  if (risks.length === 0) {
    return (
      <div className="flex items-center gap-2 rounded border border-emerald-500/20 bg-emerald-500/5 px-3 py-2 text-label text-emerald-800 dark:text-emerald-300">
        <CircleCheck size={14} />
        No control-plane risks detected in this snapshot.
      </div>
    );
  }
  return (
    <div className="space-y-2">
      {risks.map((risk) => (
        <div key={risk.id} className="flex items-start gap-2 rounded border border-amber-500/20 bg-amber-500/5 px-3 py-2">
          <AlertTriangle size={14} className="mt-0.5 flex-shrink-0 text-amber-700 dark:text-amber-300" />
          <div className="min-w-0">
            <div className="text-label font-medium text-amber-800 dark:text-amber-200">{risk.id}</div>
            <div className="text-label leading-relaxed text-gray-400">{risk.message}</div>
          </div>
        </div>
      ))}
    </div>
  );
}

function formatObjectiveLatency(value: number | null): string {
  if (value === null) return "No sample";
  if (value < 100) return `${value}ms`;
  return `${(value / 1000).toFixed(1)}s`;
}

function ObjectiveHealthMetric({
  testId,
  label,
  value,
  risk = false,
  detail,
}: {
  testId: string;
  label: string;
  value: number | string;
  risk?: boolean;
  detail?: string;
}) {
  return (
    <div
      data-testid={testId}
      data-severity={risk ? "risk" : "normal"}
      className={`rounded border p-3 ${
        risk
          ? "border-red-500/40 bg-red-500/10"
          : "border-border bg-surface-1"
      }`}
    >
      <div
        className={`text-caption uppercase tracking-wider ${
          risk ? "text-red-700 dark:text-red-300" : "text-gray-600"
        }`}
      >
        {label}
      </div>
      <div
        className={`mt-1 text-display font-semibold ${
          risk ? "text-red-800 dark:text-red-200" : "text-gray-100"
        }`}
      >
        {value}
      </div>
      {detail && <p className="mt-1 text-caption leading-relaxed text-gray-600">{detail}</p>}
    </div>
  );
}

function ObjectiveHealthPanel({ health }: { health: ObjectiveHealthSnapshot | null }) {
  if (!health || health.availability === "unavailable" || !health.metrics) {
    return (
      <div className="rounded border border-red-500/40 bg-red-500/10 p-4 text-red-800 dark:text-red-200">
        <div className="flex items-center gap-2 text-body font-semibold">
          <AlertTriangle size={16} />
          Unavailable
        </div>
        <p className="mt-2 text-label leading-relaxed text-gray-400">
          {health?.unavailable_reason ?? "Objective health has not produced an observable snapshot."}
        </p>
        <p className="mt-2 text-caption text-red-700 dark:text-red-300">
          Metrics are intentionally hidden: unavailable is not a healthy zero.
        </p>
      </div>
    );
  }

  const metrics = health.metrics;
  const releaseGateViolations =
    metrics.technical_user_handoff_violations_24h +
    metrics.avoidable_user_reprompts_24h +
    metrics.overdue_ownerless_remediations +
    metrics.stalled_system_owned_objectives +
    metrics.unavailable_domain_adapter_objectives +
    metrics.invalid_user_attention_requests +
    metrics.invalid_terminal_convergences +
    metrics.invalid_completions_24h +
    metrics.duplicate_committed_side_effect_receipts_24h +
    metrics.requested_ceiling_downgrades_24h;
  const releaseGatePassing =
    Boolean(health.build_git_sha) &&
    health.production_window_covered === true &&
    releaseGateViolations === 0;
  const guardrails = [
    {
      testId: "objective-technical-handoffs",
      label: "Technical handoff violations",
      value: metrics.technical_user_handoff_violations,
      detail: "Technical recovery incorrectly projected to the user.",
    },
    {
      testId: "objective-invalid-attention",
      label: "Invalid user attention",
      value: metrics.invalid_user_attention_requests,
      detail: "User action was requested without a complete typed input or decision payload.",
    },
    {
      testId: "objective-invalid-terminal-convergence",
      label: "Invalid terminal convergence",
      value: metrics.invalid_terminal_convergences,
      detail: "Objective terminal revision is not bound to one visible final and settled turn.",
    },
    {
      testId: "objective-ownerless-remediations",
      label: "Overdue ownerless remediation",
      value: metrics.overdue_ownerless_remediations,
      detail: "Due system work without a valid owner lease.",
    },
    {
      testId: "objective-stalled-system-owned",
      label: "Stalled system-owned",
      value: metrics.stalled_system_owned_objectives,
      detail: "No durable progress within the bounded recovery window.",
    },
    {
      testId: "objective-unavailable-adapters",
      label: "Unavailable recovery adapters",
      value: metrics.unavailable_domain_adapter_objectives,
      detail: "Open work is assigned to a registered but non-executable domain.",
    },
    {
      testId: "objective-invalid-completions",
      label: "Invalid completion",
      value: metrics.invalid_completions,
      detail: "Completed without retaining its acceptance predicate.",
    },
    {
      testId: "objective-duplicate-receipts",
      label: "Duplicate committed receipts",
      value: metrics.duplicate_committed_side_effect_receipts,
      detail: "More than one committed receipt for one action fingerprint.",
    },
  ];

  return (
    <div className="space-y-3">
      <div
        data-testid="objective-release-gate"
        data-status={releaseGatePassing ? "passing" : "blocked"}
        className={`flex flex-wrap items-center justify-between gap-2 rounded border px-3 py-2 ${
          releaseGatePassing
            ? "border-emerald-500/20 bg-emerald-500/5"
            : "border-red-500/40 bg-red-500/10"
        }`}
      >
        <div
          className={`inline-flex items-center gap-2 text-label font-medium ${
            releaseGatePassing
              ? "text-emerald-800 dark:text-emerald-300"
              : "text-red-800 dark:text-red-200"
          }`}
        >
          {releaseGatePassing ? <CircleCheck size={14} /> : <AlertTriangle size={14} />}
          {releaseGatePassing ? "24h non-interruption gate passing" : "24h non-interruption gate blocked"}
        </div>
        <div className="text-caption text-gray-600">
          {health.build_git_sha
            ? `Build ${health.build_git_sha.slice(0, 12)}`
            : "Development build · not production proof"}
          {" · "}Observed {new Date(health.generated_at_ms).toLocaleString()}
        </div>
      </div>
      {health.build_git_sha && health.production_window_covered !== true && (
        <p className="text-caption text-amber-700 dark:text-amber-300">
          Production observation window incomplete; zero counters are not yet 24h proof.
        </p>
      )}

      <div className="grid grid-cols-1 gap-2 md:grid-cols-3">
        <ObjectiveHealthMetric testId="objective-open" label="Open Objectives" value={metrics.open} />
        <ObjectiveHealthMetric
          testId="objective-system-owned"
          label="System-owned"
          value={metrics.system_owned}
        />
        <ObjectiveHealthMetric
          testId="objective-typed-attention"
          label="Typed user attention"
          value={metrics.typed_user_attention}
        />
      </div>

      <div className="grid grid-cols-1 gap-2 md:grid-cols-4">
        {guardrails.map((metric) => (
          <ObjectiveHealthMetric
            key={metric.testId}
            {...metric}
            risk={metric.value > 0}
          />
        ))}
      </div>

      <div className="grid grid-cols-1 gap-2 md:grid-cols-4">
        <ObjectiveHealthMetric
          testId="objective-24h-technical-handoffs"
          label="24h technical handoffs"
          value={metrics.technical_user_handoff_violations_24h}
          risk={metrics.technical_user_handoff_violations_24h > 0}
        />
        <ObjectiveHealthMetric
          testId="objective-24h-avoidable-reprompts"
          label="24h avoidable reprompts"
          value={metrics.avoidable_user_reprompts_24h}
          risk={metrics.avoidable_user_reprompts_24h > 0}
        />
        <ObjectiveHealthMetric
          testId="objective-24h-duplicate-receipts"
          label="24h duplicate side effects"
          value={metrics.duplicate_committed_side_effect_receipts_24h}
          risk={metrics.duplicate_committed_side_effect_receipts_24h > 0}
        />
        <ObjectiveHealthMetric
          testId="objective-24h-ceiling-downgrades"
          label="24h ceiling downgrades"
          value={metrics.requested_ceiling_downgrades_24h}
          risk={metrics.requested_ceiling_downgrades_24h > 0}
        />
      </div>

      <div className="grid grid-cols-1 gap-2 md:grid-cols-[1.5fr_1fr_1fr]">
        <ObjectiveHealthMetric
          testId="objective-recovery-24h"
          label="24h recovered / decisions"
          value={`${metrics.recovered_objectives_24h} / ${metrics.recovery_decisions_24h}`}
          detail={`Lifetime: ${metrics.recovered_objectives} recovered / ${metrics.recovery_decisions} decisions`}
        />
        <ObjectiveHealthMetric
          testId="objective-recovery-p50"
          label="24h recovery P50"
          value={formatObjectiveLatency(metrics.recovery_latency_p50_ms_24h)}
        />
        <ObjectiveHealthMetric
          testId="objective-recovery-p95"
          label="24h recovery P95"
          value={formatObjectiveLatency(metrics.recovery_latency_p95_ms_24h)}
        />
      </div>
    </div>
  );
}

export function ControlPlanePage({ onBack }: ControlPlanePageProps) {
  const activeSession = useChatStore((s) => s.activeSession);
  const cwd = activeSession?.cwd ?? null;
  const [snapshot, setSnapshot] = useState<ControlPlaneSnapshot | null>(null);
  const [objectiveHealth, setObjectiveHealth] = useState<ObjectiveHealthSnapshot | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const requestSequence = useRef(0);

  const generatedAt = useMemo(() => {
    if (!snapshot?.generated_at) return "";
    return new Date(snapshot.generated_at).toLocaleString();
  }, [snapshot?.generated_at]);

  const load = useCallback(async () => {
    const requestId = ++requestSequence.current;
    setLoading(true);
    setError(null);
    try {
      const next = await requestControlPlaneSnapshot(cwd);
      if (requestId !== requestSequence.current) return;
      const nextObjectiveHealth = await requestObjectiveHealthSnapshot();
      if (requestId !== requestSequence.current) return;
      setSnapshot(next);
      setObjectiveHealth(nextObjectiveHealth);
    } catch (e) {
      if (requestId !== requestSequence.current) return;
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      if (requestId !== requestSequence.current) return;
      setLoading(false);
    }
  }, [cwd]);

  useEffect(() => {
    void load();
    return () => {
      requestSequence.current += 1;
    };
  }, [load]);

  useEffect(() => {
    if (!error) return;
    const recoveryId = window.setTimeout(() => {
      void load();
    }, CONTROL_PLANE_RECOVERY_DELAY_MS);
    return () => window.clearTimeout(recoveryId);
  }, [error, load]);

  return (
    <div className="flex h-full flex-col bg-surface-0">
      <header className="flex items-center justify-between border-b border-border bg-surface-1 px-4 py-3">
        <div className="flex items-center gap-2 min-w-0">
          <button
            onClick={onBack}
            className="rounded p-1 text-gray-600 transition-colors hover:bg-surface-3 hover:text-gray-300"
            title="返回"
          >
            <ChevronLeft size={16} />
          </button>
          <ShieldCheck size={16} className="text-accent" />
          <div className="min-w-0">
            <h1 className="text-body font-semibold text-gray-100">AI Coding OS</h1>
            <p className="truncate text-caption text-gray-600">
              {snapshot?.cwd ?? "未绑定项目上下文"}
            </p>
          </div>
        </div>
        <button
          onClick={load}
          disabled={loading}
          className="inline-flex items-center gap-1.5 rounded border border-border px-2 py-1 text-label text-gray-400 transition-colors hover:bg-surface-3 hover:text-gray-200 disabled:opacity-50"
        >
          {loading ? <CircleDashed size={14} className="animate-spin" /> : <RefreshCcw size={14} />}
          刷新
        </button>
      </header>

      <main className="flex-1 overflow-y-auto">
        <div className="mx-auto max-w-6xl px-6 py-5">
          {error && (
              <div className="mb-4 rounded border border-red-500/30 bg-red-500/10 px-3 py-2 text-label text-red-800 dark:text-red-200">
              {error}
            </div>
          )}

          {!snapshot ? (
            <div className="flex h-64 items-center justify-center text-body text-gray-600">
              {loading ? "加载控制面…" : "暂无控制面快照"}
            </div>
          ) : (
            <>
              <div className="mb-4 flex flex-wrap items-center gap-3 text-label text-gray-500">
                <span className="inline-flex items-center gap-1">
                  <GitBranch size={14} />
                  {gitContextLabel(snapshot.delivery)}
                </span>
                <span>{generatedAt}</span>
              </div>

              <Section title="Risks">
                <RiskList risks={snapshot.risks} />
              </Section>

              <Section title="Objective Continuity">
                <ObjectiveHealthPanel health={objectiveHealth} />
              </Section>

              <Section title="Authority Surfaces">
                <AuthorityGrid items={snapshot.authority} />
              </Section>

              <Section title="Memory Lifecycle">
                <MemoryPanel memory={snapshot.memory} />
              </Section>

              <Section title="Capability Registry">
                <CapabilityGrid items={snapshot.capabilities} />
              </Section>

              <Section title="Delivery Gates">
                <DeliveryPanel delivery={snapshot.delivery} />
              </Section>
            </>
          )}
        </div>
      </main>
    </div>
  );
}
