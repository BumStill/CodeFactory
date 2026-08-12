// SPDX-License-Identifier: Apache-2.0
// Resume-journal banner: what the content-addressed journal did at scheduler
// start — restored-from-cache vs re-running vs recovered-orphan counts, with
// per-task reasons. A pure function of the ResumeReport (no IPC, no git), so
// the lock-safe headless acceptance page can drive every state from fixtures.

import { RotateCcw, History, Zap } from "lucide-react";
import type { ResumeReport } from "../stores/tasks";
import { RESUME_REASON_LABELS } from "../stores/tasks";

export function ResumeBanner({ report }: { report: ResumeReport | undefined }) {
  if (!report) return null;
  const { restored, invalidated, recovered } = report;
  if (restored.length === 0 && invalidated.length === 0 && recovered.length === 0) {
    return null;
  }

  return (
    <div
      data-testid="resume-banner"
      className="mx-3 mt-2 rounded-lg border border-border bg-surface-2 px-3 py-2 text-label"
    >
      <div className="flex items-center gap-2 text-gray-200">
        <History size={12} className="shrink-0 text-accent" />
        <span>
          已从缓存恢复 <b>{restored.length}</b> 个任务，重新执行{" "}
          <b>{invalidated.length}</b> 个，恢复中断任务 <b>{recovered.length}</b> 个
        </span>
      </div>
      {invalidated.length > 0 && (
        <ul className="mt-1.5 space-y-0.5 text-gray-500">
          {invalidated.map((t) => (
            <li key={t.task_id} className="flex items-center gap-1.5">
              <RotateCcw size={10} className="shrink-0" />
              <span className="truncate">{t.title}</span>
              <span
                className="shrink-0 rounded bg-surface-3 px-1 py-px text-caption text-amber-500"
                title={`该任务因「${RESUME_REASON_LABELS[t.reason]}」需要重新执行`}
              >
                {RESUME_REASON_LABELS[t.reason]}
              </span>
            </li>
          ))}
        </ul>
      )}
      {recovered.length > 0 && (
        <ul className="mt-1 space-y-0.5 text-gray-500">
          {recovered.map((t) => (
            <li key={t.task_id} className="flex items-center gap-1.5">
              <Zap size={10} className="shrink-0" />
              <span className="truncate">{t.title}</span>
              <span className="shrink-0 rounded bg-surface-3 px-1 py-px text-caption text-sky-500">
                {t.outcome === "finalized" ? "已确认完成" : "已恢复待执行"}
              </span>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}

/** 已缓存 badge for a restored task row (keyed lookup done by the caller). */
export function RestoredBadge({ keyShort }: { keyShort: string }) {
  return (
    <span
      className="ml-1.5 shrink-0 rounded bg-surface-3 px-1 py-px text-caption text-emerald-500"
      title={`已从缓存恢复(内容指纹 ${keyShort}) — 输入未变化,未重新执行`}
    >
      已缓存
    </span>
  );
}
