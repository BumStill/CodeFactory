// SPDX-License-Identifier: Apache-2.0
//
// Always-visible header pill mirroring the updater state. The floating
// UpdaterBanner is non-blocking and the user might dismiss it; this pill
// stays put so even a session left open for hours can see "update ready"
// the moment the next 30-min poll surfaces it. Clicking opens the install
// flow directly — no need to navigate anywhere.

import { Download, RefreshCw, Check, AlertCircle } from "lucide-react";
import {
  countUpdateBlockers,
  describeUpdateObjectiveBlockers,
  useUpdaterStore,
} from "../stores/updater";

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
        className="flex items-center gap-1.5 px-2 py-0.5 rounded-full bg-accent/15 border border-accent/40 text-caption text-accent hover:bg-accent/25 transition-colors animate-pulse motion-reduce:animate-none"
        title={`点击下载并安装 v${phase.update.version}`}
      >
        <Download size={14} />
        更新到 v{phase.update.version}
      </button>
    );
  }

  if (phase.kind === "downloading") {
    const pct = phase.total ? Math.round((phase.received / phase.total) * 100) : 0;
    return (
      <span className="flex items-center gap-1.5 px-2 py-0.5 rounded-full bg-accent/10 text-caption text-accent">
        <RefreshCw size={14} className="animate-spin motion-reduce:animate-none" />
        正在下载 {pct}%
      </span>
    );
  }

  if (phase.kind === "waiting_for_safe_restart") {
    const objectiveBlockers = describeUpdateObjectiveBlockers(phase.blockers);
    const observingUnknownInstall =
      phase.blockers?.update_install_state === "still_unknown" ||
      phase.blockers?.update_install_state === "observe_only";
    return (
      <span
        className="flex items-center gap-1.5 px-2 py-0.5 rounded-full bg-amber-500/15 text-caption text-amber-800 dark:text-amber-300"
        title={[
          observingUnknownInstall
            ? "上次安装结果尚未确认；系统只读核对，不会重复安装。"
            : "执行中的本地 session 结束后会自动安装，不会直接重启。",
          objectiveBlockers,
          phase.safetyCheckError,
        ].filter(Boolean).join(" ")}
      >
        <RefreshCw size={14} />
        {observingUnknownInstall
          ? "正在核对更新结果"
          : `等待安全更新 · ${countUpdateBlockers(phase.blockers)}`}
      </span>
    );
  }

  if (phase.kind === "installing" || phase.kind === "ready") {
    return (
      <span className="flex items-center gap-1.5 px-2 py-0.5 rounded-full bg-emerald-500/15 text-caption text-emerald-800 dark:text-emerald-300">
        <Check size={14} />
        {phase.kind === "installing" ? "安装中…" : "重启中…"}
      </span>
    );
  }

  // Idle / checking / up-to-date / error — small subdued pill showing version
  // and acting as a manual "check now" button.
  const versionText = currentVersion ? `v${currentVersion}` : "检查中…";
  const errored = phase.kind === "error";

  return (
    <button
      onClick={() => void checkNow()}
      className={`flex items-center gap-1 px-1.5 py-0.5 rounded text-caption transition-colors ${
        errored
          ? "text-rose-400 hover:bg-rose-500/10"
          : "text-gray-600 hover:text-gray-300 hover:bg-surface-3"
      }`}
      title={
        errored
          ? `上次检查失败：${phase.message}\n系统会按计划自动再次检查；点击仅用于立即检查。`
          : phase.kind === "up_to_date"
          ? `已是最新版本。\n上次检查于 ${new Date(phase.checkedAt).toLocaleTimeString()}。\n点击再次检查。`
          : phase.kind === "checking"
          ? "正在检查更新…"
          : "点击检查更新。"
      }
    >
      {errored && <AlertCircle size={14} />}
      {phase.kind === "checking" && <RefreshCw size={14} className="animate-spin motion-reduce:animate-none" />}
      {versionText}
    </button>
  );
}
