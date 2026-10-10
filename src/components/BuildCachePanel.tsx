// SPDX-License-Identifier: Apache-2.0
// CF-BLD-R4: the build cache the user can see and reclaim in one action.
//
// The numbers are the real ones the runtime enforces (one ceiling for every
// cache CodeFactory owns, least-recently-used eviction, never a cache that is
// being built into). Nothing here decides what may be deleted — it calls the
// same command the menu entry calls, so there is one policy, not two.
import { useCallback, useEffect, useState } from "react";
import { invoke } from "../lib/tauri";

export interface BuildCacheEntry {
  path: string;
  bytes: number;
  files: number;
  last_used_unix: number;
  in_use: boolean;
  owner?: string | null;
}

export interface BuildCacheReport {
  total_bytes: number;
  budget_bytes: number;
  entries: BuildCacheEntry[];
  heavy_builds_running: number;
  heavy_builds_waiting: number;
  heavy_build_limit: number;
  heavy_build_status?: string | null;
}

export interface MaintenanceOutcome {
  scanned: number;
  before_bytes: number;
  after_bytes: number;
  reclaimed_bytes: number;
  evicted_bytes: number;
  protected_bytes: number;
  overflow_bytes: number;
  removed: string[];
}

/** Sizes are shown the way the budget is described, not in raw bytes. */
export function formatGiB(bytes: number): string {
  const gib = bytes / 1024 ** 3;
  return gib >= 10 ? `${gib.toFixed(0)} GB` : `${gib.toFixed(1)} GB`;
}

export function buildCacheSummary(report: BuildCacheReport): string {
  return `编译缓存占用 ${formatGiB(report.total_bytes)}，上限 ${formatGiB(report.budget_bytes)}`;
}

/** Plain language, and it never claims to have freed more than it did. */
export function cleanupSummary(outcome: MaintenanceOutcome): string {
  const freed = outcome.reclaimed_bytes + outcome.evicted_bytes;
  if (freed === 0) {
    return `没有可以清理的编译缓存，当前占用 ${formatGiB(outcome.after_bytes)}。`;
  }
  return `已清理 ${formatGiB(freed)}，现在占用 ${formatGiB(outcome.after_bytes)}。`;
}

export function heavyBuildLine(report: BuildCacheReport): string | null {
  if (report.heavy_build_status) return report.heavy_build_status;
  if (report.heavy_builds_running > 0) {
    return `正在编译 ${report.heavy_builds_running}/${report.heavy_build_limit}。`;
  }
  return null;
}

export default function BuildCachePanel() {
  const [report, setReport] = useState<BuildCacheReport | null>(null);
  const [message, setMessage] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const refresh = useCallback(async () => {
    try {
      const next = await invoke<BuildCacheReport>("build_cache_report");
      setReport(next);
    } catch (error) {
      setMessage(`读取编译缓存失败：${String(error)}`);
    }
  }, []);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  const cleanup = useCallback(async () => {
    setBusy(true);
    try {
      const outcome = await invoke<MaintenanceOutcome>("build_cache_cleanup");
      setMessage(cleanupSummary(outcome));
      await refresh();
    } catch (error) {
      setMessage(`清理编译缓存失败：${String(error)}`);
    } finally {
      setBusy(false);
    }
  }, [refresh]);

  const queue = report ? heavyBuildLine(report) : null;

  return (
    <section
      data-testid="build-cache-panel"
      className="flex flex-col gap-3 rounded-lg border border-border bg-surface-1 p-4"
    >
      <div className="flex items-start justify-between gap-3">
        <div className="flex flex-col gap-1">
          <h3 className="text-title">编译缓存</h3>
          <p className="text-note text-muted">
            {report ? buildCacheSummary(report) : "正在读取…"}
          </p>
          {queue ? <p className="text-note text-muted">{queue}</p> : null}
        </div>
        <button
          type="button"
          data-testid="build-cache-cleanup"
          onClick={() => void cleanup()}
          disabled={busy}
          className="rounded border border-border px-3 py-1 text-note disabled:opacity-50"
        >
          一键清理
        </button>
      </div>

      {report && report.entries.length > 0 ? (
        <ul className="flex flex-col gap-1" data-testid="build-cache-entries">
          {report.entries.map((entry) => (
            <li key={entry.path} className="flex items-center justify-between gap-3 text-note">
              <span className="truncate" title={entry.path}>
                {entry.path}
              </span>
              <span className="text-muted">
                {formatGiB(entry.bytes)}
                {entry.in_use ? "（构建中）" : ""}
              </span>
            </li>
          ))}
        </ul>
      ) : null}

      {message ? (
        <p data-testid="build-cache-message" className="text-note text-muted">
          {message}
        </p>
      ) : null}
    </section>
  );
}
