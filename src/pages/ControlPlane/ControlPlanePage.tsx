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

interface ControlPlanePageProps {
  onBack: () => void;
}

const CONTROL_PLANE_REQUEST_TIMEOUT_MS = 8_000;

async function requestControlPlaneSnapshot(cwd: string | null): Promise<ControlPlaneSnapshot> {
  let timeoutId: ReturnType<typeof setTimeout> | undefined;
  const watchdog = new Promise<never>((_, reject) => {
    timeoutId = setTimeout(
      () => reject(new Error("控制面请求超过 8 秒，请重试。")),
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

export function ControlPlanePage({ onBack }: ControlPlanePageProps) {
  const activeSession = useChatStore((s) => s.activeSession);
  const cwd = activeSession?.cwd ?? null;
  const [snapshot, setSnapshot] = useState<ControlPlaneSnapshot | null>(null);
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
      setSnapshot(next);
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

  return (
    <div className="flex h-full flex-col bg-surface-0">
      <header className="flex items-center justify-between border-b border-border bg-surface-1 px-4 py-3">
        <div className="flex items-center gap-2 min-w-0">
          <button
            onClick={onBack}
            className="rounded p-1 text-gray-600 transition-colors hover:bg-surface-3 hover:text-gray-300"
            title="返回"
          >
            <ChevronLeft size={15} />
          </button>
          <ShieldCheck size={17} className="text-accent" />
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
          {loading ? <CircleDashed size={13} className="animate-spin" /> : <RefreshCcw size={13} />}
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
                  <GitBranch size={13} />
                  {gitContextLabel(snapshot.delivery)}
                </span>
                <span>{generatedAt}</span>
              </div>

              <Section title="Risks">
                <RiskList risks={snapshot.risks} />
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
