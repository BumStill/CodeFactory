// SPDX-License-Identifier: Apache-2.0
import { useEffect, useLayoutEffect, useRef, useState, type KeyboardEvent as ReactKeyboardEvent } from "react";
import { Check, ShieldAlert, X, Unlock } from "lucide-react";
import { createTwoFilesPatch } from "diff";
import type { PendingPermission } from "../stores/chat";
import { formatToolArgs } from "../stores/chatEvents";
import { DiffViewer, parseUnifiedDiffResult } from "./DiffViewer";

interface Props {
  request: PendingPermission;
  trusted: boolean;
  onAllow: () => void;
  onDeny: () => void;
  onAllowFullAccess: () => void;
}

const FOCUSABLE_SELECTOR = [
  "a[href]",
  "button:not([disabled])",
  "input:not([disabled])",
  "select:not([disabled])",
  "textarea:not([disabled])",
  "summary",
  '[tabindex]:not([tabindex="-1"])',
].join(",");

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
        <div className="mb-1 font-mono text-caption text-gray-500">
          写入 <span className="text-gray-300">{write.path}</span>
          {"（若文件已存在将被整份覆盖，以下是新内容，非差异）"}
        </div>
        <pre className="max-h-80 overflow-auto rounded border border-border bg-surface-1 p-3 text-label text-gray-300 whitespace-pre-wrap break-all">
          {write.content.slice(0, 4000)}
          {write.content.length > 4000 && "\n[truncated]"}
        </pre>
      </div>
    );
  }

  return (
    <pre className="rounded border border-border bg-surface-1 p-3 text-label text-gray-300 whitespace-pre-wrap break-all">
      {formatToolArgs(request.args)}
    </pre>
  );
}

export function PermissionDialog({
  request,
  trusted,
  onAllow,
  onDeny,
  onAllowFullAccess,
}: Props) {
  const dialogRef = useRef<HTMLDivElement>(null);
  const denyButtonRef = useRef<HTMLButtonElement>(null);
  const restoreFocusRef = useRef<HTMLElement | null>(null);
  const [remainingSeconds, setRemainingSeconds] = useState<number | null>(() =>
    request.expiresAt == null
      ? null
      : Math.max(0, Math.ceil((request.expiresAt - Date.now()) / 1000)),
  );
  useEffect(() => {
    if (request.expiresAt == null) {
      setRemainingSeconds(null);
      return;
    }
    const update = () =>
      setRemainingSeconds(Math.max(0, Math.ceil((request.expiresAt! - Date.now()) / 1000)));
    update();
    const timer = window.setInterval(update, 250);
    return () => window.clearInterval(timer);
  }, [request.expiresAt]);

  useLayoutEffect(() => {
    restoreFocusRef.current = document.activeElement instanceof HTMLElement
      ? document.activeElement
      : null;
    denyButtonRef.current?.focus();

    return () => {
      const restoreTarget = restoreFocusRef.current;
      if (restoreTarget?.isConnected) restoreTarget.focus();
    };
  }, []);

  const handleKeyDown = (event: ReactKeyboardEvent<HTMLDivElement>) => {
    if (event.key === "Escape") {
      event.preventDefault();
      event.stopPropagation();
      onDeny();
      return;
    }
    if (event.key !== "Tab") return;

    const dialog = dialogRef.current;
    if (!dialog) return;
    const focusable = Array.from(dialog.querySelectorAll<HTMLElement>(FOCUSABLE_SELECTOR))
      .filter((element) => !element.hasAttribute("disabled") && element.getAttribute("aria-hidden") !== "true");
    const first = focusable[0];
    const last = focusable[focusable.length - 1];

    if (!first || !last) {
      event.preventDefault();
      dialog.focus();
      return;
    }

    const active = document.activeElement;
    if (event.shiftKey && (active === first || !dialog.contains(active))) {
      event.preventDefault();
      last.focus();
    } else if (!event.shiftKey && (active === last || !dialog.contains(active))) {
      event.preventDefault();
      first.focus();
    }
  };

  return (
    <div
      ref={dialogRef}
      role="dialog"
      aria-modal="true"
      aria-labelledby="permission-dialog-title"
      tabIndex={-1}
      onKeyDown={handleKeyDown}
      className="fixed inset-0 z-50 flex items-center justify-center bg-black/60 px-4"
    >
      <div className="w-full max-w-lg rounded-lg border border-border bg-surface-2 shadow-2xl">
        <div className="flex items-center gap-2 border-b border-border px-4 py-3">
          <ShieldAlert size={16} className="text-amber-400" />
          <div className="min-w-0">
            <h2 id="permission-dialog-title" className="text-body font-semibold text-gray-100">需要权限</h2>
            <p className="text-label text-gray-500">
              工具 `{request.toolName}` 想要以项目访问权限运行。
            </p>
          </div>
        </div>

        <div className="max-h-[45vh] overflow-auto px-4 py-3">
          <div className="mb-2 text-label text-gray-600">参数</div>
          <ToolArgsPreview request={request} />
          {remainingSeconds != null && (
            <div className="mt-3 rounded border border-amber-500/30 bg-amber-500/10 px-3 py-2 text-label text-amber-800 dark:text-amber-200">
              请在 {remainingSeconds} 秒内处理；超时只会标记“授权已过期”，不会记成你拒绝。
            </div>
          )}
          {trusted && (
            <div className="mt-3 rounded border border-amber-500/30 bg-amber-500/10 px-3 py-2 text-label text-amber-800 dark:text-amber-200">
              当前会话已处于信任模式，普通工具会减少确认；高风险命令仍会拦截。
            </div>
          )}
        </div>

        <div className="flex flex-wrap justify-end gap-2 border-t border-border px-4 py-3">
          <button
            ref={denyButtonRef}
            onClick={onDeny}
            className="inline-flex min-h-11 items-center gap-1.5 rounded border border-border px-3 text-label text-gray-400 hover:bg-surface-3 hover:text-gray-100 lg:min-h-9"
          >
            <X size={14} />
            拒绝
          </button>
          <button
            onClick={onAllow}
            className="inline-flex min-h-11 items-center gap-1.5 rounded bg-accent px-3 text-label text-white hover:bg-accent-hover lg:min-h-9"
          >
            <Check size={14} />
            仅允许一次
          </button>
          <button
            onClick={onAllowFullAccess}
            className="inline-flex min-h-11 items-center gap-1.5 rounded border border-amber-500/40 bg-amber-500/10 px-3 text-label text-amber-900 dark:text-amber-100 hover:bg-amber-500/20 lg:min-h-9"
          >
            <Unlock size={14} />
            信任本会话并允许
          </button>
        </div>
      </div>
    </div>
  );
}
