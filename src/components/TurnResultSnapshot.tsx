// SPDX-License-Identifier: Apache-2.0

import {
  AlertTriangle,
  CheckCircle2,
  ChevronDown,
  CircleDashed,
  FileCode2,
  ListTree,
  PanelRightOpen,
  RefreshCw,
  TestTube2,
} from "lucide-react";
import { useState } from "react";
import type { TurnPlan } from "../lib/chatPlan";
import { planProgress } from "../lib/chatPlan";
import { formatDuration } from "../lib/duration";
import type { ToolCallState } from "../stores/chatEvents";

const MAX_EVIDENCE_ITEMS = 20;

export interface TurnEvidenceSummary {
  operationCount: number;
  changedFileCount: number;
  verificationCount: number;
  changedFiles: string[];
  verificationCommands: string[];
  failureCount: number;
  truncated: boolean;
}

function parseArgs(args: string): Record<string, unknown> {
  try {
    const value = JSON.parse(args) as unknown;
    return value && typeof value === "object" && !Array.isArray(value)
      ? value as Record<string, unknown>
      : {};
  } catch {
    return {};
  }
}

function pushUniqueBounded(values: string[], value: unknown): boolean {
  if (typeof value !== "string" || !value.trim() || values.includes(value)) return false;
  if (values.length >= MAX_EVIDENCE_ITEMS) return true;
  values.push(value);
  return false;
}

export function summarizeTurnEvidence(toolCalls: ToolCallState[]): TurnEvidenceSummary {
  const changedFiles: string[] = [];
  const verificationCommands: string[] = [];
  const changedFileKeys = new Set<string>();
  const verificationKeys = new Set<string>();
  let truncated = false;
  let failureCount = 0;
  for (const tool of toolCalls) {
    const args = parseArgs(tool.args);
    const succeeded = tool.status === "done" && !tool.isError;
    const failed = tool.isError || tool.status === "blocked" || tool.status === "error" || tool.status === "denied" || tool.status === "cancelled";
    if (failed) {
      failureCount += 1;
    }
    if (succeeded && (tool.name === "write_file" || tool.name === "edit_file")) {
      if (
        typeof args.path === "string" &&
        args.path.trim() &&
        !changedFileKeys.has(args.path)
      ) {
        changedFileKeys.add(args.path);
        truncated = pushUniqueBounded(changedFiles, args.path) || truncated;
      }
    }
    if (succeeded && tool.name === "bash" && typeof args.command === "string") {
      const command = args.command;
      if (
        /\b(test|build|check|lint|verify|smoke|typecheck)\b/i.test(command) &&
        !verificationKeys.has(command)
      ) {
        verificationKeys.add(command);
        truncated = pushUniqueBounded(verificationCommands, command) || truncated;
      }
    }
  }
  return {
    operationCount: toolCalls.length,
    changedFileCount: changedFileKeys.size,
    verificationCount: verificationKeys.size,
    changedFiles,
    verificationCommands,
    failureCount,
    truncated,
  };
}

interface Props {
  plan: TurnPlan;
  evidence: TurnEvidenceSummary;
  /** A provider/runtime/turn boundary failed even when no tool call did. */
  turnBoundaryFailure?: boolean;
  durationMs: number | null;
  processExpanded: boolean;
  onToggleProcess?: () => void;
  onOpenEvidence?: () => void;
  evidenceControlsId?: string;
  evidenceOpen?: boolean;
}

export function TurnResultSnapshot({
  plan,
  evidence,
  turnBoundaryFailure = false,
  durationMs,
  processExpanded,
  onToggleProcess,
  onOpenEvidence,
  evidenceControlsId,
  evidenceOpen = false,
}: Props) {
  const [resultOpen, setResultOpen] = useState(false);
  const [summaryOpen, setSummaryOpen] = useState(false);
  const progress = planProgress(plan);
  const complete = progress.total > 0 && progress.completed === progress.total;
  const effectiveFailureCount =
    evidence.failureCount + (turnBoundaryFailure && evidence.failureCount === 0 ? 1 : 0);
  const hasFailureEvidence = effectiveFailureCount > 0;
  const hasWaitingBoundary = Boolean(plan.waitingReason);
  // Legacy plans and malformed owners remain system-owned. The visible wait
  // reason is evidence, never an authorization signal.
  const nextActionOwner = plan.nextActionOwner ?? "system";
  const status = hasWaitingBoundary && nextActionOwner === "user"
    ? {
        tone: "warning",
        label: "需要你处理",
        icon: AlertTriangle,
        iconClass: "text-status-warning",
        borderClass: "border-l-status-warning",
      }
    : hasWaitingBoundary && nextActionOwner === "external"
      ? {
          tone: "neutral",
          label: "外部等待",
          icon: CircleDashed,
          iconClass: "text-gray-500",
          borderClass: "border-l-border",
        }
      : hasWaitingBoundary
        ? {
            tone: "neutral",
            label: "系统继续处理",
            icon: CircleDashed,
            iconClass: "text-gray-500",
            borderClass: "border-l-border",
          }
        : hasFailureEvidence
          ? {
              tone: "warning",
              label: complete ? "已执行，证据待复核" : "执行未完成，证据待复核",
              icon: AlertTriangle,
              iconClass: "text-status-warning",
              borderClass: "border-l-status-warning",
            }
          : complete
            ? {
                tone: "success",
                label: "已完成",
                icon: CheckCircle2,
                iconClass: "text-status-success",
                borderClass: "border-l-status-success",
              }
            : {
                tone: "neutral",
                label: "未完成",
                icon: CircleDashed,
                iconClass: "text-gray-500",
                borderClass: "border-l-border",
              };
  const StatusIcon = status.icon;
  const attentionDetail = plan.waitingReason
    ?? (turnBoundaryFailure
      ? "回合存在失败或中断证据"
      : effectiveFailureCount > 0
        ? `${effectiveFailureCount} 项操作失败或未完成`
        : null);
  const summary = `完成 ${progress.completed}/${progress.total} 个计划步骤；修改 ${evidence.changedFileCount} 个文件；记录 ${evidence.verificationCount} 项验证操作；${
    effectiveFailureCount === 0 ? "没有失败证据。" : `有 ${effectiveFailureCount} 项失败证据。`
  }`;

  return (
    <section
      data-testid="turn-result-snapshot"
      data-status-tone={status.tone}
      aria-label="任务结果"
      className={`mt-3 max-w-[72ch] overflow-hidden rounded-xl border border-border/70 border-l-2 bg-surface-2/70 ${status.borderClass}`}
    >
      <div className="flex flex-wrap items-center gap-2 px-3 py-2.5">
        <StatusIcon size={15} aria-hidden="true" className={status.iconClass} />
        <span className="text-note font-semibold text-gray-200">{status.label}</span>
        <span className="rounded-md bg-surface-3 px-1.5 py-0.5 text-caption font-medium tabular-nums text-gray-400">
          {progress.completed}/{progress.total}
        </span>
        <span className="text-caption text-gray-500">
          {evidence.operationCount} 项操作
          {durationMs != null ? ` · ${formatDuration(durationMs)}` : ""}
        </span>
        <div className="ml-auto flex flex-wrap items-center gap-1">
          <button
            type="button"
            aria-label="查看证据"
            aria-haspopup={onOpenEvidence ? "dialog" : undefined}
            aria-controls={onOpenEvidence ? evidenceControlsId : undefined}
            aria-expanded={onOpenEvidence ? evidenceOpen : resultOpen}
            onClick={() => {
              if (onOpenEvidence) {
                onOpenEvidence();
              } else {
                setResultOpen((value) => !value);
              }
            }}
            className="inline-flex min-h-11 items-center gap-1 rounded-lg px-2 text-note text-gray-400 transition-colors hover:bg-surface-3 hover:text-gray-200 lg:min-h-9"
          >
            查看证据
            {onOpenEvidence
              ? <PanelRightOpen size={12} aria-hidden="true" />
              : <ChevronDown size={12} aria-hidden="true" className={resultOpen ? "rotate-180" : ""} />}
          </button>
          {onToggleProcess && (
            <button
              type="button"
              aria-label="执行过程"
              aria-expanded={processExpanded}
              onClick={onToggleProcess}
              className="inline-flex min-h-11 items-center gap-1 rounded-lg px-2 text-note text-gray-400 transition-colors hover:bg-surface-3 hover:text-gray-200 lg:min-h-9"
            >
              <ListTree size={12} aria-hidden="true" />
              执行过程
            </button>
          )}
          <button
            type="button"
            aria-label="结果摘要"
            aria-expanded={summaryOpen}
            onClick={() => setSummaryOpen((value) => !value)}
            className="inline-flex min-h-11 items-center gap-1 rounded-lg px-2 text-note text-gray-400 transition-colors hover:bg-surface-3 hover:text-gray-200 lg:min-h-9"
          >
            <RefreshCw size={12} aria-hidden="true" />
            结果摘要
          </button>
        </div>
      </div>

      {attentionDetail && (
        <p role="status" className="border-t border-border/50 bg-status-warning-soft px-3 py-2 text-note leading-5 text-status-warning">
          当前边界 · {attentionDetail}
        </p>
      )}

      {resultOpen && (
        <div className="grid gap-3 border-t border-border/50 px-3 py-2.5 text-note sm:grid-cols-2">
          <div>
            <p className="mb-1 flex items-center gap-1 text-gray-400">
              <FileCode2 size={12} aria-hidden="true" />
              修改文件
            </p>
            {evidence.changedFiles.length > 0 ? (
              <ul className="space-y-0.5 font-mono text-label text-gray-300">
                {evidence.changedFiles.map((path) => <li key={path} className="truncate">{path}</li>)}
              </ul>
            ) : <p className="text-gray-600">没有文件修改证据</p>}
          </div>
          <div>
            <p className="mb-1 flex items-center gap-1 text-gray-400">
              <TestTube2 size={12} aria-hidden="true" />
              验证
            </p>
            {evidence.verificationCommands.length > 0 ? (
              <ul className="space-y-0.5 font-mono text-label text-gray-300">
                {evidence.verificationCommands.map((command) => <li key={command} className="truncate">{command}</li>)}
              </ul>
            ) : <p className="text-gray-600">没有验证命令证据</p>}
          </div>
          <div className="sm:col-span-2">
            <p className="mb-1 text-gray-400">等待与失败边界</p>
            {(plan.waitingHistory?.length ?? 0) > 0 ? (
              <ul className="space-y-0.5 text-gray-300">
                {plan.waitingHistory?.map((reason) => (
                  <li key={reason}>等待 · {reason}</li>
                ))}
              </ul>
            ) : (
              <p className="text-gray-600">没有等待原因证据</p>
            )}
            <p
              className={
                effectiveFailureCount > 0
                  ? "mt-1 text-status-danger"
                  : "mt-1 text-gray-600"
              }
            >
              {effectiveFailureCount > 0
                ? `${effectiveFailureCount} 项失败或中断证据`
                : "没有失败操作证据"}
            </p>
          </div>
          {evidence.truncated && (
            <p className="text-caption text-gray-600 sm:col-span-2">仅显示前 {MAX_EVIDENCE_ITEMS} 项；完整证据仍在执行过程。</p>
          )}
        </div>
      )}

      {summaryOpen && (
        <p role="status" className="border-t border-border/50 px-3 py-2 text-note leading-5 text-gray-300">
          {summary}
        </p>
      )}
    </section>
  );
}
