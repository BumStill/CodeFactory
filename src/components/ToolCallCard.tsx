// SPDX-License-Identifier: Apache-2.0
import { useState } from "react";
import { ChevronDown, ChevronRight, Terminal, AlertCircle, CheckCircle, ShieldQuestion } from "lucide-react";
import type { ToolCallState } from "../stores/chat";
import { DiffViewer, parseUnifiedDiffResult } from "./DiffViewer";

interface Props {
  tc: ToolCallState;
}

export function ToolCallCard({ tc }: Props) {
  const [open, setOpen] = useState(false);
  const parsedDiff = tc.result == null ? null : parseUnifiedDiffResult(tc.result);
  const hasDiff = (parsedDiff?.files.length ?? 0) > 0;

  const statusIcon =
    tc.status === "waiting_permission" ? (
      <ShieldQuestion size={12} className="text-amber-400 shrink-0" />
    ) : tc.status === "done" && !tc.isError ? (
      <CheckCircle size={12} className="text-green-500 shrink-0" />
    ) : tc.status === "error" || tc.status === "denied" || tc.isError ? (
      <AlertCircle size={12} className="text-red-400 shrink-0" />
    ) : (
      <span className="w-3 h-3 rounded-full border border-accent animate-pulse shrink-0" />
    );

  return (
    <div className="my-1 rounded border border-border bg-surface-2 text-xs font-mono">
      <button
        className="flex w-full items-center gap-2 px-3 py-1.5 text-left hover:bg-surface-3 transition-colors"
        onClick={() => setOpen((o) => !o)}
      >
        {open ? <ChevronDown size={12} /> : <ChevronRight size={12} />}
        <Terminal size={12} className="text-accent shrink-0" />
        <span className="text-gray-300 truncate">{tc.name}</span>
        <span className="rounded bg-surface-4 px-1.5 py-0.5 text-[10px] uppercase tracking-wide text-gray-500">
          {labelForStatus(tc.status)}
        </span>
        <span className="ml-auto">{statusIcon}</span>
      </button>

      {open && (
        <div className="border-t border-border px-3 py-2 space-y-2">
          {tc.args && (
            <div>
              <div className="text-gray-500 mb-1">input</div>
              <pre className="text-gray-300 whitespace-pre-wrap break-all">{formatArgs(tc.args)}</pre>
            </div>
          )}
          {tc.result != null && (
            <div>
              <div className={`mb-1 ${tc.isError ? "text-red-400" : "text-gray-500"}`}>
                {tc.isError ? "error" : "output"}
              </div>
              {hasDiff ? (
                <DiffViewer output={tc.result} />
              ) : (
                <pre className={`whitespace-pre-wrap break-all ${tc.isError ? "text-red-300" : "text-gray-300"}`}>
                  {tc.result.slice(0, 2000)}
                  {tc.result.length > 2000 && "\n[truncated]"}
                </pre>
              )}
            </div>
          )}
        </div>
      )}
    </div>
  );
}

function labelForStatus(status: ToolCallState["status"]): string {
  switch (status) {
    case "waiting_permission":
      return "waiting";
    case "running":
      return "running";
    case "done":
      return "done";
    case "error":
      return "error";
    case "denied":
      return "denied";
  }
}

function formatArgs(raw: string): string {
  try {
    return JSON.stringify(JSON.parse(raw), null, 2);
  } catch {
    return raw;
  }
}
