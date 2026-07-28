// SPDX-License-Identifier: Apache-2.0
import { Check, ShieldAlert, X, Unlock } from "lucide-react";
import { createTwoFilesPatch } from "diff";
import type { PendingPermission } from "../stores/chat";
import { formatToolArgs } from "../stores/chatEvents";
import { DiffViewer, parseUnifiedDiffResult } from "./DiffViewer";

interface Props {
  request: PendingPermission;
  fullAccess: boolean;
  onAllow: () => void;
  onDeny: () => void;
  onAllowFullAccess: () => void;
}

/** `edit_file`'s exact args shape (see src-tauri/src/tools/edit.rs). */
function asEditArgs(args: unknown): { path: string; old_string: string; new_string: string } | null {
  if (typeof args !== "object" || args === null) return null;
  const a = args as Record<string, unknown>;
  if (typeof a.path === "string" && typeof a.old_string === "string" && typeof a.new_string === "string") {
    return { path: a.path, old_string: a.old_string, new_string: a.new_string };
  }
  return null;
}

/** `write_file`'s exact args shape (see src-tauri/src/tools/write.rs). */
function asWriteArgs(args: unknown): { path: string; content: string } | null {
  if (typeof args !== "object" || args === null) return null;
  const a = args as Record<string, unknown>;
  if (typeof a.path === "string" && typeof a.content === "string") {
    return { path: a.path, content: a.content };
  }
  return null;
}

/** A real diff of the proposed change — we have both old_string and new_string up front. */
function editFileDiff(path: string, oldString: string, newString: string) {
  const patch = createTwoFilesPatch(path, path, oldString, newString, "", "");
  const firstHunk = patch.indexOf("--- ");
  return parseUnifiedDiffResult(firstHunk >= 0 ? patch.slice(firstHunk) : patch);
}

function ToolArgsPreview({ request }: { request: PendingPermission }) {
  const edit = request.toolName === "edit_file" ? asEditArgs(request.args) : null;
  if (edit) {
    const parsed = editFileDiff(edit.path, edit.old_string, edit.new_string);
    if (parsed.files.length > 0) {
      return <DiffViewer output="" parsed={parsed} />;
    }
  }

  const write = request.toolName === "write_file" ? asWriteArgs(request.args) : null;
  if (write) {
    return (
      <div>
        <div className="mb-1 font-mono text-[11px] text-gray-500">
          写入 <span className="text-gray-300">{write.path}</span>
          {"（若文件已存在将被整份覆盖，以下是新内容，非差异）"}
        </div>
        <pre className="max-h-80 overflow-auto rounded border border-border bg-surface-1 p-3 text-xs text-gray-300 whitespace-pre-wrap break-all">
          {write.content.slice(0, 4000)}
          {write.content.length > 4000 && "\n[truncated]"}
        </pre>
      </div>
    );
  }

  return (
    <pre className="rounded border border-border bg-surface-1 p-3 text-xs text-gray-300 whitespace-pre-wrap break-all">
      {formatToolArgs(request.args)}
    </pre>
  );
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
            <h2 className="text-sm font-semibold text-gray-100">需要权限</h2>
            <p className="text-xs text-gray-500">
              工具 `{request.toolName}` 想要以项目访问权限运行。
            </p>
          </div>
        </div>

        <div className="max-h-[45vh] overflow-auto px-4 py-3">
          <div className="mb-2 text-xs uppercase tracking-wide text-gray-600">参数</div>
          <ToolArgsPreview request={request} />
          {fullAccess && (
            <div className="mt-3 rounded border border-amber-500/30 bg-amber-500/10 px-3 py-2 text-xs text-amber-800 dark:text-amber-200">
              已启用完全访问，后续匹配的工具调用将不再提示。
            </div>
          )}
        </div>

        <div className="flex flex-wrap justify-end gap-2 border-t border-border px-4 py-3">
          <button
            onClick={onDeny}
            className="inline-flex items-center gap-1.5 rounded border border-border px-3 py-1.5 text-xs text-gray-400 hover:bg-surface-3 hover:text-gray-100"
          >
            <X size={13} />
            拒绝
          </button>
          <button
            onClick={onAllow}
            className="inline-flex items-center gap-1.5 rounded bg-accent px-3 py-1.5 text-xs text-white hover:bg-accent-hover"
          >
            <Check size={13} />
            仅允许一次
          </button>
          <button
            onClick={onAllowFullAccess}
            className="inline-flex items-center gap-1.5 rounded border border-amber-500/40 bg-amber-500/10 px-3 py-1.5 text-xs text-amber-900 dark:text-amber-100 hover:bg-amber-500/20"
          >
            <Unlock size={13} />
            完全访问并允许
          </button>
        </div>
      </div>
    </div>
  );
}
