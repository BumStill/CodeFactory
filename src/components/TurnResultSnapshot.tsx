// SPDX-License-Identifier: Apache-2.0

import {
  AlertTriangle,
  CheckCircle2,
  ChevronDown,
  CircleDashed,
  FileCode2,
  PanelRightOpen,
  RefreshCw,
  TestTube2,
} from "lucide-react";
import { useState } from "react";
import type { TurnPlan } from "../lib/chatPlan";
import { planProgress } from "../lib/chatPlan";
import { formatDuration } from "../lib/duration";
import type { ToolCallState } from "../stores/chatEvents";
import { humanWaitingReason } from "../lib/waitingReason";
import { lineStatsText, toolCallLineStats } from "../lib/editLineStats";
import { LineChangeStats } from "./LineChangeStats";

const MAX_EVIDENCE_ITEMS = 20;

/** A pull request the turn actually delivered, so the card can point at it. */
export interface TurnPullRequest {
  number: number;
  url: string;
}

export interface TurnEvidenceSummary {
  operationCount: number;
  changedFileCount: number;
  /** Lines added and removed by this turn's successful edits, in total. */
  addedLines: number;
  removedLines: number;
  verificationCount: number;
  changedFiles: string[];
  verificationCommands: string[];
  failureCount: number;
  /** Where the work already landed, when the turn opened or reused a PR. */
  pullRequest: TurnPullRequest | null;
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

function redactEvidenceText(value: string): string {
  return value
    .replace(
      /(^|[\s/?&])([A-Za-z_][A-Za-z0-9_-]*)=([^\s&]+)/gi,
      (match, prefix: string, name: string) =>
        /(key|token|secret|password|passwd|credential)/i.test(name)
          ? `${prefix}${name}=[redacted]`
          : match,
    )
    .replace(/\bBearer\s+[A-Za-z0-9._~+/-]+=*/gi, "Bearer [redacted]")
    .replace(/\b(?:sk|rk|pk)[-_][A-Za-z0-9_-]{6,}\b/gi, "[redacted]");
}

/**
 * Read the delivered pull request out of a `deliver_changes` tool call. The
 * card answers "where is the work" with the PR link when one exists, so it
 * must survive a session reload — the tool result and metadata are persisted
 * with the turn.
 */
function extractPullRequest(tool: ToolCallState): TurnPullRequest | null {
  const metadata = tool.metadata ?? null;
  if (metadata && typeof metadata === "object") {
    const url = typeof metadata.pr_url === "string" ? metadata.pr_url : null;
    const number = typeof metadata.pr_number === "number"
      ? metadata.pr_number
      : typeof metadata.pull_request_number === "number"
        ? metadata.pull_request_number
        : null;
    if (url && number) return { number, url };
  }
  const haystack = [tool.result, tool.args].filter(Boolean).join("\n");
  const match = haystack.match(
    /https:\/\/[^\s"'<>]*\/pull\/(\d+)/,
  );
  if (!match) return null;
  return { number: Number(match[1]), url: match[0].replace(/[.,)]+$/, "") };
}

export function summarizeTurnEvidence(toolCalls: ToolCallState[]): TurnEvidenceSummary {
  const changedFiles: string[] = [];
  const verificationCommands: string[] = [];
  const changedFileKeys = new Set<string>();
  const verificationKeys = new Set<string>();
  let truncated = false;
  let failureCount = 0;
  let addedLines = 0;
  let removedLines = 0;
  let pullRequest: TurnPullRequest | null = null;
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
        truncated = pushUniqueBounded(changedFiles, redactEvidenceText(args.path)) || truncated;
      }
    }
    // The turn total counts only edits that actually landed: a failed,
    // denied or cancelled edit contributes nothing.
    if (succeeded) {
      const stats = toolCallLineStats(tool.name, tool.args ?? "", tool.result);
      if (stats?.kind === "edit") {
        addedLines += stats.added;
        removedLines += stats.removed;
      } else if (stats?.kind === "write" && stats.newFile) {
        addedLines += stats.added;
      }
    }
    if (succeeded && tool.name === "bash" && typeof args.command === "string") {
      const command = args.command;
      if (
        /\b(test|build|check|lint|verify|smoke|typecheck)\b/i.test(command) &&
        !verificationKeys.has(command)
      ) {
        verificationKeys.add(command);
        truncated = pushUniqueBounded(
          verificationCommands,
          redactEvidenceText(command),
        ) || truncated;
      }
    }
    if (succeeded && tool.name === "deliver_changes") {
      pullRequest = extractPullRequest(tool) ?? pullRequest;
    }
  }
  return {
    operationCount: toolCalls.length,
    changedFileCount: changedFileKeys.size,
    addedLines,
    removedLines,
    verificationCount: verificationKeys.size,
    changedFiles,
    verificationCommands,
    failureCount,
    pullRequest,
    truncated,
  };
}

/** The four outcomes the card can confess to, in plain words. */
type Verdict = "failed" | "completed" | "waiting" | "incomplete";

const WAITING_OBJECTIVE_STATUSES = new Set([
  "waiting_system",
  "waiting_core_input",
  "waiting_authorization",
  "waiting_business_decision",
]);

interface Props {
  plan: TurnPlan;
  evidence: TurnEvidenceSummary;
  /**
   * The backend's authoritative terminal state for the objective that owns
   * this turn. The verdict comes from here — never from plan-step counts or
   * tool-error counts, which mislead (a blocked call the agent recovered from
   * is not a failure).
   */
  objectiveStatus?: string | null;
  /** A provider/runtime/turn boundary failed even when no tool call did. */
  turnBoundaryFailure?: boolean;
  durationMs: number | null;
  onOpenEvidence?: () => void;
  evidenceControlsId?: string;
  evidenceOpen?: boolean;
}

export function TurnResultSnapshot({
  plan,
  evidence,
  objectiveStatus = null,
  turnBoundaryFailure = false,
  durationMs,
  onOpenEvidence,
  evidenceControlsId,
  evidenceOpen = false,
}: Props) {
  const [resultOpen, setResultOpen] = useState(false);
  const [summaryOpen, setSummaryOpen] = useState(false);
  const progress = planProgress(plan);
  const complete = progress.total > 0 && progress.completed === progress.total;
  const hasWaitingBoundary = Boolean(plan.waitingReason);
  // Legacy plans and malformed owners remain system-owned. The visible wait
  // reason is evidence, never an authorization signal.
  const nextActionOwner = plan.nextActionOwner ?? "system";
  const failed = objectiveStatus === "failed";
  const objectiveCompleted = objectiveStatus === "completed";
  const objectiveWaiting = Boolean(
    objectiveStatus && WAITING_OBJECTIVE_STATUSES.has(objectiveStatus),
  );
  const verdict: Verdict = failed
    ? "failed"
    : objectiveCompleted
      ? "completed"
      : hasWaitingBoundary || objectiveWaiting
        ? "waiting"
        : turnBoundaryFailure
          // A durable boundary failure (provider error / interrupted turn) is
          // terminal evidence too, and the message above already says so.
          ? "incomplete"
          : complete
            ? "completed"
            : "incomplete";

  const status = verdict === "failed"
    ? {
        tone: "warning",
        label: "没做成",
        icon: AlertTriangle,
        iconClass: "text-status-warning",
        borderClass: "border-l-status-warning",
      }
    : verdict === "waiting"
      ? nextActionOwner === "user"
        ? {
            tone: "warning",
            label: "需要你处理",
            icon: AlertTriangle,
            iconClass: "text-status-warning",
            borderClass: "border-l-status-warning",
          }
        : nextActionOwner === "external"
          ? {
              tone: "neutral",
              label: "外部等待",
              icon: CircleDashed,
              iconClass: "text-gray-500",
              borderClass: "border-l-border",
            }
          : {
              tone: "neutral",
              label: "系统处理中",
              icon: CircleDashed,
              iconClass: "text-gray-500",
              borderClass: "border-l-border",
            }
      : verdict === "completed"
        ? {
            tone: "success",
            label: "已完成",
            icon: CheckCircle2,
            iconClass: "text-status-success",
            borderClass: "border-l-status-success",
          }
        : {
            tone: "neutral",
            label: "还没做完",
            icon: CircleDashed,
            iconClass: "text-gray-500",
            borderClass: "border-l-border",
          };
  const StatusIcon = status.icon;
  // A plan count is only meaningful once the agent actually tracked steps.
  // An all-pending plan reads as "0/5" even when the work is done, so hide it.
  const planTracked = plan.steps.some((step) => step.status !== "pending");
  const { pullRequest } = evidence;

  const waitingDetail = humanWaitingReason(plan.waitingReason);
  const noteText = verdict === "failed"
    ? null
    : waitingDetail
      ?? (verdict === "incomplete"
        ? "这次没有全部做完。回复「继续」可以接着做。"
        : null);

  const totalLineChange = evidence.addedLines > 0 || evidence.removedLines > 0;
  const openChanges = () => {
    if (onOpenEvidence) onOpenEvidence();
    else setResultOpen((value) => !value);
  };

  const summary = [
    planTracked ? `完成 ${progress.completed}/${progress.total} 个计划步骤` : null,
    `改动 ${evidence.changedFileCount} 个文件`,
    `运行 ${evidence.verificationCount} 项检查`,
    evidence.failureCount === 0
      ? "没有失败的操作。"
      : `有 ${evidence.failureCount} 项操作没有成功。`,
  ].filter(Boolean).join("；");

  return (
    <section
      data-testid="turn-result-snapshot"
      data-status-tone={status.tone}
      data-verdict={verdict}
      aria-label="任务结果"
      className={`mt-3 max-w-[72ch] overflow-hidden rounded-xl border border-border/70 border-l-2 bg-surface-2/70 ${status.borderClass}`}
    >
      <div className="flex flex-wrap items-center gap-2 px-3 py-2.5">
        <StatusIcon size={16} aria-hidden="true" className={status.iconClass} />
        <span className="text-note font-semibold text-gray-200">{status.label}</span>
        {planTracked && (
          <span className="rounded-lg bg-surface-3 px-1.5 py-0.5 text-caption font-medium tabular-nums text-gray-400">
            {progress.completed}/{progress.total}
          </span>
        )}
        <span className="text-caption text-gray-500">
          {evidence.operationCount} 项操作
          {durationMs != null ? ` · ${formatDuration(durationMs)}` : ""}
        </span>
        <div className="ml-auto flex flex-wrap items-center gap-1">
          <button
            type="button"
            aria-label="查看改动"
            aria-haspopup={onOpenEvidence ? "dialog" : undefined}
            aria-controls={onOpenEvidence ? evidenceControlsId : undefined}
            aria-expanded={onOpenEvidence ? evidenceOpen : resultOpen}
            onClick={openChanges}
            className="inline-flex min-h-11 items-center gap-1 rounded-lg px-2 text-note text-gray-400 transition-colors hover:bg-surface-3 hover:text-gray-200 lg:min-h-9"
          >
            查看改动
            {onOpenEvidence
              ? <PanelRightOpen size={14} aria-hidden="true" />
              : <ChevronDown size={14} aria-hidden="true" className={resultOpen ? "rotate-180" : ""} />}
          </button>
          <button
            type="button"
            aria-label="结果摘要"
            aria-expanded={summaryOpen}
            onClick={() => setSummaryOpen((value) => !value)}
            className="inline-flex min-h-11 items-center gap-1 rounded-lg px-2 text-note text-gray-400 transition-colors hover:bg-surface-3 hover:text-gray-200 lg:min-h-9"
          >
            <RefreshCw size={14} aria-hidden="true" />
            结果摘要
          </button>
        </div>
      </div>

      {/* How much this turn changed, in one line — the question the user
          actually asked. Nothing changed ⇒ nothing to say. */}
      {totalLineChange && (
        <button
          type="button"
          data-testid="turn-line-summary"
          aria-label={`查看改动 · 本次改了 ${evidence.changedFileCount} 个文件 ${lineStatsText({ added: evidence.addedLines, removed: evidence.removedLines })}`}
          aria-haspopup={onOpenEvidence ? "dialog" : undefined}
          aria-controls={onOpenEvidence ? evidenceControlsId : undefined}
          aria-expanded={onOpenEvidence ? evidenceOpen : resultOpen}
          onClick={openChanges}
          className="flex min-h-11 w-full items-center gap-2 border-t border-border/50 px-3 py-1.5 text-left text-note text-gray-400 transition-colors hover:bg-surface-3 hover:text-gray-200 lg:min-h-9"
        >
          <span>本次改了 {evidence.changedFileCount} 个文件</span>
          <LineChangeStats stats={{ added: evidence.addedLines, removed: evidence.removedLines }} />
        </button>
      )}

      {noteText && (
        <p role="status" className="border-t border-border/50 bg-status-warning-soft px-3 py-2 text-note leading-5 text-status-warning">
          {noteText}
        </p>
      )}

      {pullRequest && (
        <div
          role="status"
          data-testid="turn-result-where-work"
          className="space-y-1 border-t border-border/50 px-3 py-2 text-note leading-5 text-gray-300"
        >
          <p>
            改动已经交付到{" "}
            <a
              href={pullRequest.url}
              target="_blank"
              rel="noreferrer"
              className="font-medium text-status-info underline underline-offset-2"
            >
              PR #{pullRequest.number}
            </a>
            。
          </p>
        </div>
      )}

      {(verdict === "failed" || verdict === "incomplete") && !pullRequest && (
        <div
          role="status"
          data-testid="turn-result-where-work"
          className="space-y-1 border-t border-border/50 px-3 py-2 text-note leading-5 text-gray-300"
        >
          {evidence.changedFileCount > 0 ? (
            <p>
              已经改了 {evidence.changedFileCount} 个文件，改动保存在本次会话的工作目录里。
              回复「继续」把这些改动交付。
            </p>
          ) : (
            <p>没有留下文件改动。回复「继续」可以再试一次。</p>
          )}
        </div>
      )}

      {resultOpen && (
        <div className="grid gap-3 border-t border-border/50 px-3 py-2.5 text-note sm:grid-cols-2">
          <div>
            <p className="mb-1 flex items-center gap-1 text-gray-400">
              <FileCode2 size={14} aria-hidden="true" />
              改动的文件
            </p>
            {evidence.changedFiles.length > 0 ? (
              <ul className="space-y-0.5 font-mono text-label text-gray-300">
                {evidence.changedFiles.map((path) => <li key={path} className="truncate">{path}</li>)}
              </ul>
            ) : <p className="text-gray-600">没有改动文件</p>}
          </div>
          <div>
            <p className="mb-1 flex items-center gap-1 text-gray-400">
              <TestTube2 size={14} aria-hidden="true" />
              运行的检查
            </p>
            {evidence.verificationCommands.length > 0 ? (
              <ul className="space-y-0.5 font-mono text-label text-gray-300">
                {evidence.verificationCommands.map((command) => <li key={command} className="truncate">{command}</li>)}
              </ul>
            ) : <p className="text-gray-600">没有运行过检查</p>}
          </div>
          <div className="sm:col-span-2">
            <p className="mb-1 text-gray-400">等待</p>
            {(plan.waitingHistory?.length ?? 0) > 0 ? (
              <ul className="space-y-0.5 text-gray-300">
                {plan.waitingHistory?.map((reason) => (
                  <li key={reason}>等待 · {reason}</li>
                ))}
              </ul>
            ) : (
              <p className="text-gray-600">没有等待过</p>
            )}
            <p
              className={
                evidence.failureCount > 0
                  ? "mt-1 text-status-danger"
                  : "mt-1 text-gray-600"
              }
            >
              {evidence.failureCount > 0
                ? `${evidence.failureCount} 项操作没有成功`
                : "没有失败的操作"}
            </p>
          </div>
          {evidence.truncated && (
            <p className="text-caption text-gray-600 sm:col-span-2">仅显示前 {MAX_EVIDENCE_ITEMS} 项。</p>
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
