// SPDX-License-Identifier: Apache-2.0
import { Check, ShieldAlert, X, Unlock } from "lucide-react";
import type { PendingPermission } from "../stores/chat";
import { formatToolArgs } from "../stores/chatEvents";

interface Props {
  request: PendingPermission;
  fullAccess: boolean;
  onAllow: () => void;
  onDeny: () => void;
  onAllowFullAccess: () => void;
}

export function PermissionDialog({
  request,
  fullAccess,
  onAllow,
  onDeny,
  onAllowFullAccess,
}: Props) {
  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/60 px-4">
      <div className="w-full max-w-lg rounded-lg border border-border bg-surface-2 shadow-2xl">
        <div className="flex items-center gap-2 border-b border-border px-4 py-3">
          <ShieldAlert size={16} className="text-amber-400" />
          <div className="min-w-0">
            <h2 className="text-sm font-semibold text-gray-100">Permission required</h2>
            <p className="text-xs text-gray-500">
              Tool `{request.toolName}` wants to run with project access.
            </p>
          </div>
        </div>

        <div className="max-h-[45vh] overflow-auto px-4 py-3">
          <div className="mb-2 text-xs uppercase tracking-wide text-gray-600">Arguments</div>
          <pre className="rounded border border-border bg-surface-1 p-3 text-xs text-gray-300 whitespace-pre-wrap break-all">
            {formatToolArgs(request.args)}
          </pre>
          {fullAccess && (
            <div className="mt-3 rounded border border-amber-500/30 bg-amber-500/10 px-3 py-2 text-xs text-amber-200">
              Full access is enabled. Future matching tool calls can run without this prompt.
            </div>
          )}
        </div>

        <div className="flex flex-wrap justify-end gap-2 border-t border-border px-4 py-3">
          <button
            onClick={onDeny}
            className="inline-flex items-center gap-1.5 rounded border border-border px-3 py-1.5 text-xs text-gray-400 hover:bg-surface-3 hover:text-gray-100"
          >
            <X size={13} />
            Deny
          </button>
          <button
            onClick={onAllow}
            className="inline-flex items-center gap-1.5 rounded bg-accent px-3 py-1.5 text-xs text-white hover:bg-accent-hover"
          >
            <Check size={13} />
            Allow once
          </button>
          <button
            onClick={onAllowFullAccess}
            className="inline-flex items-center gap-1.5 rounded border border-amber-500/40 bg-amber-500/10 px-3 py-1.5 text-xs text-amber-100 hover:bg-amber-500/20"
          >
            <Unlock size={13} />
            Full access and allow
          </button>
        </div>
      </div>
    </div>
  );
}
