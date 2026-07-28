// SPDX-License-Identifier: Apache-2.0

import { CheckCircle2, ChevronDown, FileCode2, ListTree, RefreshCw, TestTube2 } from "lucide-react";
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
    if (tool.isError || tool.status === "error" || tool.status === "denied" || tool.status === "cancelled") {
      failureCount += 1;
    }
    if (tool.name === "write_file" || tool.name === "edit_file") {
      if (
        typeof args.path === "string" &&
        args.path.trim() &&
        !changedFileKeys.has(args.path)
      ) {
        changedFileKeys.add(args.path);
        truncated = pushUniqueBounded(changedFiles, args.path) || truncated;
      }
    }
    if (tool.name === "bash" && typeof args.command === "string") {
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
  durationMs: number | null;
  processExpanded: boolean;
  onToggleProcess?: () => void;
}

export function TurnResultSnapshot({
  plan,
  evidence,
  durationMs,
  processExpanded,
  onToggleProcess,
}: Props) {
  const [resultOpen, setResultOpen] = useState(false);
  const [summaryOpen, setSummaryOpen] = useState(false);
  const progress = planProgress(plan);
  const complete = progress.total > 0 && progress.completed === progress.total;
  const summary = `完成 ${progress.completed}/${progress.total} 个计划步骤；修改 ${evidence.changedFileCount} 个文件；执行 ${evidence.verificationCount} 项验证；${
    evidence.failureCount === 0 ? "没有失败证据。" : `有 ${evidence.failureCount} 项失败证据。`
  }`;

  return (
    <section
      data-testid="turn-result-snapshot"
      aria-label="任务结果"
      className="mt-3 max-w-[72ch] rounded-xl border border-border/60 bg-surface-1/50"
    >
      <div className="flex flex-wrap items-center gap-2 px-3 py-2">
        <CheckCircle2 size={14} aria-hidden="true" className={complete ? "text-green-500" : "text-amber-500"} />
        <span className="text-[13px] font-medium text-gray-200">任务结果</span>
        <span className="text-[11px] text-gray-500">
          {progress.completed}/{progress.total} 个步骤 · {evidence.operationCount} 项操作
          {durationMs != null ? ` · ${formatDuration(durationMs)}` : ""}
        </span>
        <div className="ml-auto flex flex-wrap items-center gap-1">
          <button
            type="button"
            aria-label="结果视图"
            aria-expanded={resultOpen}
            onClick={() => setResultOpen((value) => !value)}
            className="inline-flex min-h-7 items-center gap-1 rounded px-2 text-[11px] text-gray-400 hover:bg-surface-3 hover:text-gray-200"
          >
            结果视图
            <ChevronDown size={12} aria-hidden="true" className={resultOpen ? "rotate-180" : ""} />
          </button>
          {onToggleProcess && (
            <button
              type="button"
              aria-label="完整过程"
              aria-pressed={processExpanded}
              onClick={onToggleProcess}
              className="inline-flex min-h-7 items-center gap-1 rounded px-2 text-[11px] text-gray-400 hover:bg-surface-3 hover:text-gray-200"
            >
              <ListTree size={12} aria-hidden="true" />
              完整过程
            </button>
          )}
          <button
            type="button"
            aria-label="证据化重新总结"
            aria-expanded={summaryOpen}
            onClick={() => setSummaryOpen((value) => !value)}
            className="inline-flex min-h-7 items-center gap-1 rounded px-2 text-[11px] text-gray-400 hover:bg-surface-3 hover:text-gray-200"
          >
            <RefreshCw size={12} aria-hidden="true" />
            证据化重新总结
          </button>
        </div>
      </div>

      {resultOpen && (
        <div className="grid gap-3 border-t border-border/50 px-3 py-2 text-[12px] sm:grid-cols-2">
          <div>
            <p className="mb-1 flex items-center gap-1 text-gray-400">
              <FileCode2 size={12} aria-hidden="true" />
              修改文件
            </p>
            {evidence.changedFiles.length > 0 ? (
              <ul className="space-y-0.5 font-mono text-[11px] text-gray-300">
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
              <ul className="space-y-0.5 font-mono text-[11px] text-gray-300">
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
                evidence.failureCount > 0
                  ? "mt-1 text-red-700 dark:text-red-300"
                  : "mt-1 text-gray-600"
              }
            >
              {evidence.failureCount > 0
                ? `${evidence.failureCount} 项操作失败或未完成`
                : "没有失败操作证据"}
            </p>
          </div>
          {evidence.truncated && (
            <p className="text-[11px] text-gray-600 sm:col-span-2">仅显示前 {MAX_EVIDENCE_ITEMS} 项；完整证据仍在执行过程。</p>
          )}
        </div>
      )}

      {summaryOpen && (
        <p role="status" className="border-t border-border/50 px-3 py-2 text-[12px] leading-5 text-gray-300">
          {summary}
        </p>
      )}
    </section>
  );
}
