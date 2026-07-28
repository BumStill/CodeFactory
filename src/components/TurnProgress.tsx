// SPDX-License-Identifier: Apache-2.0

import { ChevronDown, Clock3, GitBranch, Loader2 } from "lucide-react";
import { useMemo, useState } from "react";
import type {
  ExternalJobState,
  TurnPlan,
  TurnTimingProfile,
} from "../lib/chatPlan";
import { planProgress } from "../lib/chatPlan";
import { estimateTurnRemaining } from "../lib/turnEstimate";
import { formatDuration } from "../lib/duration";

interface Props {
  plan: TurnPlan;
  timingProfile: TurnTimingProfile | null;
  externalJobs: ExternalJobState[];
  elapsedMs: number;
  nowMs?: number;
}

function estimateLabel(lowMs: number, highMs: number): string {
  const low = Math.max(1, Math.ceil(lowMs / 60_000));
  const high = Math.max(low, Math.ceil(highMs / 60_000));
  return `预计还需 ${low}–${high} 分钟`;
}

const STEP_KIND_LABELS: Record<string, string> = {
  analysis: "分析",
  implementation: "实现",
  verification: "构建/验证",
  delivery: "交付",
  external_job: "外部任务",
  other: "其他",
};

const JOB_STATUS_LABELS: Record<string, string> = {
  pending: "等待中",
  queued: "排队中",
  running: "运行中",
  completed: "已完成",
  failed: "失败",
  cancelled: "已取消",
  interrupted: "已中断",
};

export function TurnProgress({
  plan,
  timingProfile,
  externalJobs,
  elapsedMs,
  nowMs = Date.now(),
}: Props) {
  const [expanded, setExpanded] = useState(false);
  const progress = useMemo(() => planProgress(plan), [plan]);
  const estimate = useMemo(
    () => estimateTurnRemaining(plan, timingProfile, externalJobs, nowMs),
    [externalJobs, nowMs, plan, timingProfile],
  );
  const minimumSampleCount = estimate
    ? Math.min(...estimate.sources.map((source) => source.sampleCount))
    : null;
  const linkedExternalJob =
    progress.current?.kind === "external_job" && progress.current.externalJobId
      ? externalJobs.find((job) => job.id === progress.current?.externalJobId)
      : null;

  return (
    <section
      data-testid="turn-progress"
      aria-label="任务执行路线"
      className="w-[min(560px,calc(100vw-2rem))] rounded-xl border border-border/60 bg-surface-1/95 shadow-lg backdrop-blur"
    >
      <div className="flex items-start gap-2 px-3 py-2">
        <Loader2 size={14} aria-hidden="true" className="shrink-0 animate-spin text-accent motion-reduce:animate-none" />
        <div className="min-w-0 flex-1">
          <div className="flex min-w-0 flex-wrap items-center gap-x-2 gap-y-0.5 text-[13px]">
            <span className="shrink-0 font-medium text-gray-200">
              已完成 {progress.completed}/{progress.total}
            </span>
            <span className="shrink-0 tabular-nums text-gray-400">
              {progress.percent}%
            </span>
            <span className="shrink-0 text-[11px] text-gray-600">
              来自 {progress.total} 个计划步骤
            </span>
            <span className="ml-auto inline-flex shrink-0 items-center gap-1 text-[11px] text-gray-500 tabular-nums">
              <Clock3 size={11} aria-hidden="true" />
              {formatDuration(elapsedMs)}
            </span>
          </div>
          <div className="mt-1 grid min-w-0 grid-cols-2 gap-2 text-[12px] text-gray-400">
            <span className="truncate">
              {progress.current ? `当前 · ${progress.current.title}` : "当前 · 正在整理执行路线"}
            </span>
            <span className="truncate">
              {progress.next ? `下一步 · ${progress.next.title}` : "下一步 · 待计划更新"}
            </span>
          </div>
          {(estimate || linkedExternalJob) && (
            <div className="mt-1 flex flex-wrap gap-x-3 gap-y-0.5 text-[11px] text-gray-500">
              {estimate && minimumSampleCount != null && (
                <span>
                  {estimateLabel(estimate.lowMs, estimate.highMs)} · 最少 {minimumSampleCount} 个历史样本
                </span>
              )}
              {linkedExternalJob && (
                <span>
                  外部任务 · {JOB_STATUS_LABELS[linkedExternalJob.status] ?? linkedExternalJob.status}
                </span>
              )}
            </div>
          )}
          <div
            role="progressbar"
            aria-label={`任务进度，来自 ${progress.total} 个计划步骤`}
            aria-valuemin={0}
            aria-valuemax={100}
            aria-valuenow={progress.percent}
            className="mt-1.5 h-1 overflow-hidden rounded-full bg-surface-3"
          >
            <div
              className="h-full rounded-full bg-accent transition-[width] duration-300 motion-reduce:transition-none"
              style={{ width: `${progress.percent}%` }}
            />
          </div>
        </div>
        <button
          type="button"
          aria-label={expanded ? "收起执行路线" : "展开执行路线"}
          aria-expanded={expanded}
          onClick={() => setExpanded((value) => !value)}
          className="rounded p-1 text-gray-500 transition-colors hover:bg-surface-3 hover:text-gray-200"
        >
          <ChevronDown
            size={14}
            aria-hidden="true"
            className={`transition-transform motion-reduce:transition-none ${expanded ? "rotate-180" : ""}`}
          />
        </button>
      </div>

      {expanded && (
        <div className="space-y-2 border-t border-border/50 px-3 py-2 text-[12px]">
          <ol className="space-y-1" aria-label="计划步骤">
            {plan.steps.map((step, index) => (
              <li key={step.id} className="flex min-w-0 items-center gap-2">
                <span
                  aria-hidden="true"
                  className={`h-1.5 w-1.5 shrink-0 rounded-full ${
                    step.status === "completed"
                      ? "bg-green-500"
                      : step.status === "in_progress"
                        ? "bg-accent"
                        : "bg-gray-600"
                  }`}
                />
                <span className={step.status === "pending" ? "text-gray-500" : "text-gray-300"}>
                  {index + 1}. {step.title}
                </span>
              </li>
            ))}
          </ol>
          <div className="flex flex-wrap gap-x-4 gap-y-1 text-gray-500">
            {progress.next && <span>下一步 · {progress.next.title}</span>}
            {estimate && (
              <span>
                时间样本 ·{" "}
                {estimate.sources
                  .map(
                    (source) =>
                      `${STEP_KIND_LABELS[source.kind] ?? source.kind} ${source.sampleCount}`,
                  )
                  .join(" + ")}
              </span>
            )}
          </div>
          {plan.waitingReason && (
            <p role="status" className="rounded border-l-2 border-amber-500/60 bg-amber-500/5 px-2 py-1 text-amber-700 dark:text-amber-200">
              等待 · {plan.waitingReason}
            </p>
          )}
          {plan.changeReason && (
            <p className="flex items-start gap-1.5 text-gray-400">
              <GitBranch size={12} aria-hidden="true" className="mt-0.5 shrink-0" />
              计划已调整 · {plan.changeReason}
            </p>
          )}
        </div>
      )}
    </section>
  );
}
