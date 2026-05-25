// SPDX-License-Identifier: Apache-2.0
//
// Always-visible header pill mirroring the updater state. The floating
// UpdaterBanner is non-blocking and the user might dismiss it; this pill
// stays put so even a session left open for hours can see "update ready"
// the moment the next 30-min poll surfaces it. Clicking opens the install
// flow directly — no need to navigate anywhere.

import { Download, RefreshCw, Check, AlertCircle } from "lucide-react";
import { useUpdaterStore } from "../stores/updater";

export function UpdateStatusPill() {
  const phase = useUpdaterStore((s) => s.phase);
  const currentVersion = useUpdaterStore((s) => s.currentVersion);
  const install = useUpdaterStore((s) => s.install);
  const checkNow = useUpdaterStore((s) => s.checkNow);

  // Available — the "you should click me" state, full accent pill.
  if (phase.kind === "available") {
    return (
      <button
        onClick={() => void install()}
        className="flex items-center gap-1.5 px-2 py-0.5 rounded-full bg-accent/15 border border-accent/40 text-[11px] text-accent hover:bg-accent/25 transition-colors animate-pulse"
        title={`Click to download and install v${phase.update.version}`}
      >
        <Download size={11} />
        Update to v{phase.update.version}
      </button>
    );
  }

  if (phase.kind === "downloading") {
    const pct = phase.total ? Math.round((phase.received / phase.total) * 100) : 0;
    return (
      <span className="flex items-center gap-1.5 px-2 py-0.5 rounded-full bg-accent/10 text-[11px] text-accent">
        <RefreshCw size={11} className="animate-spin" />
        Downloading {pct}%
      </span>
    );
  }

  if (phase.kind === "installing" || phase.kind === "ready") {
    return (
      <span className="flex items-center gap-1.5 px-2 py-0.5 rounded-full bg-emerald-500/15 text-[11px] text-emerald-800 dark:text-emerald-300">
        <Check size={11} />
        {phase.kind === "installing" ? "Installing…" : "Restarting…"}
      </span>
    );
  }

  // Idle / checking / up-to-date / error — small subdued pill showing version
  // and acting as a manual "check now" button.
  const versionText = currentVersion ? `v${currentVersion}` : "checking…";
  const errored = phase.kind === "error";

  return (
    <button
      onClick={() => void checkNow()}
      className={`flex items-center gap-1 px-1.5 py-0.5 rounded text-[10px] transition-colors ${
        errored
          ? "text-rose-400 hover:bg-rose-500/10"
          : "text-gray-600 hover:text-gray-300 hover:bg-surface-3"
      }`}
      title={
        errored
          ? `Last check failed: ${phase.message}\nClick to retry.`
          : phase.kind === "up_to_date"
          ? `Up to date.\nLast checked ${new Date(phase.checkedAt).toLocaleTimeString()}.\nClick to check again.`
          : phase.kind === "checking"
          ? "Checking for updates…"
          : "Click to check for updates."
      }
    >
      {errored && <AlertCircle size={10} />}
      {phase.kind === "checking" && <RefreshCw size={10} className="animate-spin" />}
      {versionText}
    </button>
  );
}
