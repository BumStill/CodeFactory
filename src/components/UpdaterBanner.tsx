// SPDX-License-Identifier: Apache-2.0
import { useEffect, useState } from "react";
import { check, type Update } from "@tauri-apps/plugin-updater";
import { relaunch } from "@tauri-apps/plugin-process";
import { Download, X, RefreshCw, CheckCircle, AlertCircle } from "lucide-react";

type Phase =
  | { kind: "idle" }
  | { kind: "available"; update: Update }
  | { kind: "downloading"; received: number; total: number | null }
  | { kind: "installing" }
  | { kind: "ready" }
  | { kind: "error"; message: string };

/**
 * Background updater banner.
 * - On mount: silently checks the configured endpoint
 * - If newer version available: shows a non-blocking banner
 * - User clicks Update → download with progress → install → relaunch
 * - User clicks Dismiss → hidden for this session (re-check next launch)
 */
export function UpdaterBanner() {
  const [phase, setPhase] = useState<Phase>({ kind: "idle" });
  const [dismissed, setDismissed] = useState(false);

  useEffect(() => {
    // Skip in dev (updater endpoint isn't reachable for unsigned dev builds anyway).
    // Use a runtime check via window.__TAURI_INTERNALS__ rather than import.meta.env
    // so we don't need to wire vite/client types here.
    if ((import.meta as { env?: { DEV?: boolean } }).env?.DEV) return;

    let cancelled = false;
    (async () => {
      try {
        const update = await check();
        if (cancelled) return;
        if (update?.available) {
          setPhase({ kind: "available", update });
        }
      } catch (err) {
        // Network errors or no-update-available are silent — don't alarm the user.
        // Only log so dev can see it via DevTools.
        console.warn("[updater] check failed:", err);
      }
    })();

    return () => {
      cancelled = true;
    };
  }, []);

  if (dismissed) return null;
  if (phase.kind === "idle") return null;

  const handleInstall = async () => {
    if (phase.kind !== "available") return;
    const update = phase.update;
    try {
      setPhase({ kind: "downloading", received: 0, total: null });
      await update.downloadAndInstall((event) => {
        switch (event.event) {
          case "Started":
            setPhase({ kind: "downloading", received: 0, total: event.data.contentLength ?? null });
            break;
          case "Progress":
            setPhase((p) =>
              p.kind === "downloading"
                ? { ...p, received: p.received + event.data.chunkLength }
                : p,
            );
            break;
          case "Finished":
            setPhase({ kind: "installing" });
            break;
        }
      });
      setPhase({ kind: "ready" });
      // Give the user a beat to see "Restarting..." before relaunch
      setTimeout(() => relaunch().catch(console.error), 800);
    } catch (err) {
      setPhase({
        kind: "error",
        message: err instanceof Error ? err.message : String(err),
      });
    }
  };

  return (
    <div className="fixed top-3 right-3 z-50 w-80 rounded-lg border border-accent/40 bg-surface-2 shadow-2xl overflow-hidden">
      {phase.kind === "available" && (
        <>
          <div className="flex items-start gap-2 p-3">
            <Download size={14} className="text-accent shrink-0 mt-0.5" />
            <div className="flex-1 min-w-0">
              <div className="text-xs font-medium text-gray-200">
                Update available: v{phase.update.version}
              </div>
              {phase.update.body && (
                <div className="mt-1 text-[11px] text-gray-500 line-clamp-3 whitespace-pre-wrap">
                  {phase.update.body}
                </div>
              )}
            </div>
            <button
              onClick={() => setDismissed(true)}
              className="p-0.5 text-gray-600 hover:text-gray-300 transition-colors"
              title="Dismiss until next launch"
            >
              <X size={12} />
            </button>
          </div>
          <div className="flex border-t border-border">
            <button
              onClick={() => setDismissed(true)}
              className="flex-1 py-1.5 text-[11px] text-gray-500 hover:bg-surface-3 transition-colors"
            >
              Later
            </button>
            <button
              onClick={handleInstall}
              className="flex-1 py-1.5 text-[11px] text-accent font-medium hover:bg-accent/10 transition-colors border-l border-border"
            >
              Install now
            </button>
          </div>
        </>
      )}

      {phase.kind === "downloading" && (
        <div className="p-3 space-y-2">
          <div className="flex items-center gap-2 text-xs text-gray-300">
            <RefreshCw size={12} className="text-accent animate-spin" />
            <span>Downloading update…</span>
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
          <div className="text-[10px] text-gray-600 text-right tabular-nums">
            {(phase.received / 1024 / 1024).toFixed(1)} MB
            {phase.total && ` / ${(phase.total / 1024 / 1024).toFixed(1)} MB`}
          </div>
        </div>
      )}

      {phase.kind === "installing" && (
        <div className="flex items-center gap-2 p-3 text-xs text-gray-300">
          <RefreshCw size={12} className="text-accent animate-spin" />
          <span>Installing…</span>
        </div>
      )}

      {phase.kind === "ready" && (
        <div className="flex items-center gap-2 p-3 text-xs text-green-400">
          <CheckCircle size={12} />
          <span>Installed. Restarting…</span>
        </div>
      )}

      {phase.kind === "error" && (
        <div className="p-3 space-y-2">
          <div className="flex items-start gap-2 text-xs text-red-400">
            <AlertCircle size={12} className="shrink-0 mt-0.5" />
            <span className="flex-1 break-words">Update failed: {phase.message}</span>
          </div>
          <button
            onClick={() => setDismissed(true)}
            className="w-full py-1 text-[11px] text-gray-500 hover:bg-surface-3 transition-colors rounded"
          >
            Dismiss
          </button>
        </div>
      )}
    </div>
  );
}
