// SPDX-License-Identifier: Apache-2.0
import { useEffect } from "react";
import { Download, X, RefreshCw, CheckCircle, AlertCircle } from "lucide-react";
import { countUpdateBlockers, useUpdaterStore } from "../stores/updater";

/**
 * Floating update banner — appears top-right when a new version is
 * available. Mounts the global updater poll on first render. Re-checks
 * automatically every 30 min in addition to the manual button in
 * Settings → About.
 */
export function UpdaterBanner() {
  const phase = useUpdaterStore((s) => s.phase);
  const dismissedVersion = useUpdaterStore((s) => s.dismissedVersion);
  const initialize = useUpdaterStore((s) => s.initialize);
  const install = useUpdaterStore((s) => s.install);
  const dismiss = useUpdaterStore((s) => s.dismiss);

  useEffect(() => { void initialize(); }, [initialize]);

  // Banner is visible only during user-actionable phases. "checking" /
  // "up_to_date" / "idle" / "error" stay in the header pill instead so the
  // banner doesn't flicker every poll cycle.
  const visible =
    (phase.kind === "available" && phase.update.version !== dismissedVersion) ||
    phase.kind === "downloading" ||
    phase.kind === "waiting_for_safe_restart" ||
    phase.kind === "installing" ||
    phase.kind === "ready";
  if (!visible) return null;

  return (
    <div className="fixed top-3 right-3 z-50 w-80 rounded-lg border border-accent/40 bg-surface-2 shadow-2xl overflow-hidden">
      {phase.kind === "available" && (
        <>
          <div className="flex items-start gap-2 p-3">
            <Download size={14} className="text-accent shrink-0 mt-0.5" />
            <div className="flex-1 min-w-0">
              <div className="text-label font-medium text-gray-200">
                有可用更新：v{phase.update.version}
              </div>
              {phase.update.body && (
                <div className="mt-1 text-caption text-gray-500 line-clamp-3 whitespace-pre-wrap">
                  {phase.update.body}
                </div>
              )}
            </div>
            <button
              onClick={dismiss}
              className="p-0.5 text-gray-600 hover:text-gray-300 transition-colors"
              title="忽略此版本"
            >
              <X size={14} />
            </button>
          </div>
          <div className="flex border-t border-border">
            <button
              onClick={dismiss}
              className="flex-1 py-1.5 text-caption text-gray-500 hover:bg-surface-3 transition-colors"
            >
              稍后
            </button>
            <button
              onClick={() => void install()}
              className="flex-1 py-1.5 text-caption text-accent font-medium hover:bg-accent/10 transition-colors border-l border-border"
            >
              立即安装
            </button>
          </div>
        </>
      )}

      {phase.kind === "downloading" && (
        <div className="p-3 space-y-2">
          <div className="flex items-center gap-2 text-label text-gray-300">
            <RefreshCw size={14} className="text-accent animate-spin motion-reduce:animate-none" />
            <span>正在下载更新…</span>
          </div>
          <div className="h-1 bg-surface-3 rounded-full overflow-hidden">
            <div
              className="h-full bg-accent transition-all duration-300"
              style={{
                width: phase.total
                  ? `${Math.min(100, (phase.received / phase.total) * 100)}%`
                  : "30%",
              }}
            />
          </div>
          <div className="text-caption text-gray-600 text-right tabular-nums">
            {(phase.received / 1024 / 1024).toFixed(1)} MB
            {phase.total && ` / ${(phase.total / 1024 / 1024).toFixed(1)} MB`}
          </div>
        </div>
      )}

      {phase.kind === "installing" && (
        <div className="flex items-center gap-2 p-3 text-label text-gray-300">
          <RefreshCw size={14} className="text-accent animate-spin motion-reduce:animate-none" />
          <span>安装中…</span>
        </div>
      )}

      {phase.kind === "waiting_for_safe_restart" && (
        <div className="p-3 space-y-1 text-label text-amber-700 dark:text-amber-300">
          <div className="flex items-center gap-2">
            <RefreshCw size={14} />
            <span>更新已下载，等待执行中的 session 到达安全点…</span>
          </div>
          <p className="text-caption text-gray-500">
            当前 {countUpdateBlockers(phase.blockers)} 项本地工作仍在运行；归零后会自动安装，无需再次操作。
          </p>
        </div>
      )}

      {phase.kind === "ready" && (
        <div className="flex items-center gap-2 p-3 text-label text-green-400">
          <CheckCircle size={14} />
          <span>已安装，正在重启…</span>
        </div>
      )}

      {/* error phase intentionally not shown in the floating banner — it
          sits in the header pill so transient network errors don't yank
          attention with a giant popup. */}
      {false && phase.kind === "error" && (
        <div className="p-3 flex items-start gap-2 text-label text-red-400">
          <AlertCircle size={14} className="shrink-0 mt-0.5" />
          <span className="flex-1 break-words">更新失败</span>
        </div>
      )}
    </div>
  );
}
