// SPDX-License-Identifier: Apache-2.0
import { useEffect, useMemo, useState } from "react";
import {
  AlertTriangle,
  ChevronLeft,
  CircleCheck,
  FileSearch,
  Play,
  RefreshCcw,
} from "lucide-react";
import {
  importBenchmarkResults,
  listBenchmarkProfiles,
  previewBenchmarkProviderBridge,
  probeBenchmarkEnvironment,
  startBenchmarkProviderRun,
} from "../../lib/tauri";
import type {
  BenchmarkEnvironmentProbe,
  BenchmarkProbeStatus,
  BenchmarkProviderBridgePreview,
  BenchmarkProviderBridgeRequest,
  BenchmarkProviderRunResult,
  BenchmarkTrialRecord,
  ImportedBenchmarkRun,
} from "../../lib/tauri";

interface BenchmarksPageProps {
  onBack: () => void;
}

const PROFILE_ID = "terminal-bench-2.1";
const DEFAULT_SUBSET = [
  "write-compressor",
  "extract-elf",
  "filter-js-from-html",
  "nginx-request-logging",
  "circuit-fibsqrt",
  "configure-git-webserver",
  "mteb-retrieve",
  "sanitize-git-repo",
  "query-optimize",
  "count-dataset-tokens",
  "install-windows-3.11",
  "protein-assembly",
  "build-cython-ext",
  "kv-store-grpc",
  "sparql-university",
  "torch-tensor-parallelism",
  "caffe-cifar-10",
  "qemu-startup",
];

function StatusBadge({ status }: { status: BenchmarkProbeStatus | "blocked" | "failed" | "completed" }) {
  const tone =
    status === "ok" || status === "completed"
      ? "border-emerald-500/30 bg-emerald-500/10 text-emerald-800 dark:text-emerald-300"
      : status === "warning" || status === "blocked"
        ? "border-amber-500/30 bg-amber-500/10 text-amber-800 dark:text-amber-300"
        : "border-red-500/30 bg-red-500/10 text-red-800 dark:text-red-300";
  return (
    <span className={`inline-flex items-center rounded border px-1.5 py-0.5 text-[10px] font-medium ${tone}`}>
      {status}
    </span>
  );
}

function ScorePanel({ imported }: { imported: ImportedBenchmarkRun | null }) {
  const score = useMemo(() => {
    if (!imported) return null;
    const total = imported.trials.length;
    const pass = imported.trials.filter((trial) => trial.reward > 0).length;
    const mean = total === 0
      ? 0
      : imported.trials.reduce((sum, trial) => sum + trial.reward, 0) / total;
    const failureCounts = new Map<string, number>();
    for (const trial of imported.trials) {
      const key = trial.failure_reason || trial.failure_class || (trial.reward > 0 ? "pass" : "unknown");
      failureCounts.set(key, (failureCounts.get(key) || 0) + 1);
    }
    return {
      total,
      pass,
      mean,
      failureCounts: [...failureCounts.entries()].sort((a, b) => b[1] - a[1]),
    };
  }, [imported]);

  if (!score) {
    return <p className="text-xs text-gray-600">No imported benchmark result selected.</p>;
  }

  return (
    <div className="grid grid-cols-1 gap-3 lg:grid-cols-[260px_1fr]">
      <div className="rounded border border-border bg-surface-1 p-3">
        <div className="text-[10px] uppercase tracking-wider text-gray-600">Score</div>
        <div className="mt-1 text-2xl font-semibold text-gray-100">
          {score.pass}
          <span className="text-sm font-normal text-gray-600"> / {score.total}</span>
        </div>
        <p className="mt-1 text-xs text-gray-500">mean reward {score.mean.toFixed(6)}</p>
      </div>
      <div className="rounded border border-border bg-surface-1 p-3">
        <div className="mb-2 text-xs font-medium text-gray-400">Failure reasons</div>
        <div className="grid grid-cols-2 gap-2 md:grid-cols-4">
          {score.failureCounts.map(([name, count]) => (
            <div key={name} className="rounded border border-border bg-surface-0 px-2 py-1.5">
              <div className="truncate text-[10px] text-gray-500" title={name}>{name}</div>
              <div className="text-sm font-semibold text-gray-200">{count}</div>
            </div>
          ))}
        </div>
      </div>
    </div>
  );
}

function TrialTable({ trials }: { trials: BenchmarkTrialRecord[] }) {
  if (trials.length === 0) return null;
  return (
    <div className="overflow-x-auto rounded border border-border">
      <table className="min-w-full text-left text-xs">
        <thead className="bg-surface-1 text-gray-500">
          <tr>
            <th className="px-3 py-2 font-medium">Task</th>
            <th className="px-3 py-2 font-medium">Reward</th>
            <th className="px-3 py-2 font-medium">Failure class</th>
            <th className="px-3 py-2 font-medium">Failure reason</th>
          </tr>
        </thead>
        <tbody className="divide-y divide-border">
          {trials.slice(0, 80).map((trial) => (
            <tr key={trial.id} className="bg-surface-0">
              <td className="px-3 py-2 font-mono text-gray-300">{trial.task_name}</td>
              <td className="px-3 py-2 text-gray-300">{trial.reward}</td>
              <td className="px-3 py-2 text-gray-500">{trial.failure_class || "pass"}</td>
              <td className="px-3 py-2 text-gray-500">{trial.failure_reason || "pass"}</td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

export function BenchmarksPage({ onBack }: BenchmarksPageProps) {
  const [probe, setProbe] = useState<BenchmarkEnvironmentProbe | null>(null);
  const [preview, setPreview] = useState<BenchmarkProviderBridgePreview | null>(null);
  const [authorizationPhrase, setAuthorizationPhrase] = useState("");
  const [runResult, setRunResult] = useState<BenchmarkProviderRunResult | null>(null);
  const [importPath, setImportPath] = useState("");
  const [imported, setImported] = useState<ImportedBenchmarkRun | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const bridgeRequest: BenchmarkProviderBridgeRequest = useMemo(() => ({
    profile_id: PROFILE_ID,
    task_names: DEFAULT_SUBSET,
    task_limit: DEFAULT_SUBSET.length,
    concurrency: 4,
  }), []);

  const refresh = async () => {
    setBusy(true);
    setError(null);
    try {
      const profiles = await listBenchmarkProfiles();
      const profile = profiles.find((item) => item.id === PROFILE_ID);
      if (!profile) throw new Error(`${PROFILE_ID} profile is not available`);
      const [nextProbe, nextPreview] = await Promise.all([
        probeBenchmarkEnvironment(PROFILE_ID),
        previewBenchmarkProviderBridge(bridgeRequest),
      ]);
      setProbe(nextProbe);
      setPreview(nextPreview);
      setImportPath(nextPreview.job_path);
    } catch (err) {
      setError(String(err));
    } finally {
      setBusy(false);
    }
  };

  useEffect(() => {
    void refresh();
  }, []);

  const startRun = async () => {
    setBusy(true);
    setError(null);
    setRunResult(null);
    try {
      const result = await startBenchmarkProviderRun(bridgeRequest, authorizationPhrase);
      setRunResult(result);
      if (result.imported) setImported(result.imported);
    } catch (err) {
      setError(String(err));
    } finally {
      setBusy(false);
    }
  };

  const importJob = async () => {
    if (!importPath.trim()) return;
    setBusy(true);
    setError(null);
    try {
      setImported(await importBenchmarkResults(importPath.trim()));
    } catch (err) {
      setError(String(err));
    } finally {
      setBusy(false);
    }
  };

  const blocker = runResult?.blocker || error;

  return (
    <div className="h-full overflow-y-auto bg-surface-0">
      <header className="sticky top-0 z-10 flex items-center justify-between border-b border-border bg-surface-1 px-6 py-4">
        <div className="flex min-w-0 items-center gap-3">
          <button
            onClick={onBack}
            className="rounded p-1.5 text-gray-500 hover:bg-surface-3 hover:text-gray-200"
            title="返回"
          >
            <ChevronLeft size={16} />
          </button>
          <div className="min-w-0">
            <h1 className="text-base font-semibold text-gray-200">Terminal-Bench 2.1</h1>
            <p className="truncate text-xs text-gray-500">CodeFactory agent capability evaluation</p>
          </div>
        </div>
        <button
          onClick={refresh}
          disabled={busy}
          className="inline-flex items-center gap-2 rounded border border-border px-3 py-1.5 text-xs text-gray-300 hover:bg-surface-2 disabled:opacity-50"
        >
          <RefreshCcw size={13} />
          Refresh
        </button>
      </header>

      <main className="mx-auto max-w-6xl space-y-5 px-6 py-6">
        {blocker && (
          <div className="flex items-start gap-2 rounded border border-amber-500/30 bg-amber-500/10 p-3 text-xs text-amber-800 dark:text-amber-300">
            <AlertTriangle size={14} className="mt-0.5 shrink-0" />
            <div className="min-w-0">
              <div className="font-medium">Benchmark blocker</div>
              <div className="mt-1 break-words">{blocker}</div>
            </div>
          </div>
        )}

        <section className="grid grid-cols-1 gap-3 lg:grid-cols-3">
          <div className="rounded border border-border bg-surface-1 p-3">
            <div className="mb-2 flex items-center justify-between">
              <div className="text-xs font-medium text-gray-300">Environment</div>
              {probe && <StatusBadge status={probe.ready ? "ok" : "missing"} />}
            </div>
            <div className="space-y-2">
              {probe?.items.map((item) => (
                <div key={item.id} className="flex items-start justify-between gap-2 text-xs">
                  <div className="min-w-0">
                    <div className="text-gray-300">{item.label}</div>
                    <div className="break-words text-[10px] text-gray-600">{item.detail}</div>
                  </div>
                  <StatusBadge status={item.status} />
                </div>
              ))}
            </div>
          </div>

          <div className="rounded border border-border bg-surface-1 p-3 lg:col-span-2">
            <div className="mb-2 flex items-center justify-between">
              <div className="text-xs font-medium text-gray-300">Provider bridge</div>
              {preview && <StatusBadge status={preview.ready ? "ok" : "missing"} />}
            </div>
            {preview && (
              <div className="grid grid-cols-2 gap-2 text-xs md:grid-cols-4">
                <Metric label="endpoint" value={preview.endpoint_name} />
                <Metric label="model" value={preview.model} />
                <Metric label="tasks" value={String(preview.task_limit)} />
                <Metric label="concurrency" value={String(preview.concurrency)} />
              </div>
            )}
            {preview && (
              <pre className="mt-3 max-h-28 overflow-auto rounded border border-border bg-surface-0 p-2 text-[10px] text-gray-500">
                {preview.command_preview}
              </pre>
            )}
          </div>
        </section>

        <section className="grid grid-cols-1 gap-3 lg:grid-cols-2">
          <div className="rounded border border-border bg-surface-1 p-3">
            <div className="mb-3 text-xs font-medium text-gray-300">Start fixed subset</div>
            <label className="block text-[10px] uppercase tracking-wider text-gray-600">
              Authorization phrase
            </label>
            <input
              value={authorizationPhrase}
              onChange={(event) => setAuthorizationPhrase(event.target.value)}
              placeholder={preview?.authorization_phrase || ""}
              className="mt-1 w-full rounded border border-border bg-surface-0 px-2 py-1.5 text-xs text-gray-200 outline-none focus:border-accent"
            />
            <button
              onClick={startRun}
              disabled={busy || !preview?.ready || authorizationPhrase.trim() !== preview.authorization_phrase}
              className="mt-3 inline-flex items-center gap-2 rounded border border-border px-3 py-1.5 text-xs text-gray-300 hover:bg-surface-2 disabled:opacity-50"
            >
              <Play size={13} />
              Run 18-task subset
            </button>
            {runResult && (
              <div className="mt-3 rounded border border-border bg-surface-0 p-2 text-xs">
                <div className="flex items-center justify-between">
                  <span className="text-gray-400">Run status</span>
                  <StatusBadge status={runResult.status as "blocked" | "failed" | "completed"} />
                </div>
                <div className="mt-1 text-gray-600">failure kind: {runResult.failure_kind || "none"}</div>
              </div>
            )}
          </div>

          <div className="rounded border border-border bg-surface-1 p-3">
            <div className="mb-3 text-xs font-medium text-gray-300">Import Harbor job</div>
            <input
              value={importPath}
              onChange={(event) => setImportPath(event.target.value)}
              className="w-full rounded border border-border bg-surface-0 px-2 py-1.5 font-mono text-xs text-gray-200 outline-none focus:border-accent"
            />
            <button
              onClick={importJob}
              disabled={busy || !importPath.trim()}
              className="mt-3 inline-flex items-center gap-2 rounded border border-border px-3 py-1.5 text-xs text-gray-300 hover:bg-surface-2 disabled:opacity-50"
            >
              <FileSearch size={13} />
              Import result
            </button>
          </div>
        </section>

        <section className="space-y-3 border-t border-border pt-4">
          <div className="flex items-center gap-2 text-xs font-medium text-gray-300">
            <CircleCheck size={14} />
            Latest result
          </div>
          <ScorePanel imported={imported} />
          <TrialTable trials={imported?.trials || []} />
        </section>
      </main>
    </div>
  );
}

function Metric({ label, value }: { label: string; value: string }) {
  return (
    <div className="min-w-0 rounded border border-border bg-surface-0 px-2 py-1.5">
      <div className="text-[10px] uppercase tracking-wider text-gray-600">{label}</div>
      <div className="truncate text-xs font-medium text-gray-200" title={value}>{value}</div>
    </div>
  );
}
