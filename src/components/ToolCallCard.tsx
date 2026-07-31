// SPDX-License-Identifier: Apache-2.0
import { memo, useMemo, useState } from "react";
import {
  ChevronDown, ChevronRight,
  AlertCircle, Ban, CheckCircle, ShieldQuestion,
  FileText, Edit3, Save, TerminalSquare, Search, FolderTree,
  Globe, Wrench, Bot, BookOpen, ExternalLink,
} from "lucide-react";
import type { LucideIcon } from "lucide-react";
import { useChatStore } from "../stores/chat";
import type { ToolCallState } from "../stores/chat";
import { invoke } from "../lib/tauri";
import { DiffViewer, parseUnifiedDiffResult } from "./DiffViewer";

interface Props {
  tc: ToolCallState;
}

// ── Per-tool styling ────────────────────────────────────────────────────────
// Maps the tool name to an icon + accent color. Unknown tools fall back to
// a generic wrench. Keep the icon set lean — the goal is visual diff between
// "read", "edit/write" and "execute", not encyclopedia coverage.
interface ToolStyle {
  icon: LucideIcon;
  iconClass: string;
}

interface KnowledgeSource {
  chunk_id?: string;
  document_id?: string;
  path?: string;
  title?: string | null;
  kind?: string;
  page?: number | null;
  slide?: number | null;
  heading?: string | null;
  snippet?: string;
  score?: number;
}

function styleForTool(name: string): ToolStyle {
  switch (name) {
    case "read_file":
    case "read":
      return { icon: FileText, iconClass: "text-status-info" };
    case "write_file":
    case "write":
      return { icon: Save, iconClass: "text-accent" };
    case "edit_file":
    case "edit":
      return { icon: Edit3, iconClass: "text-accent" };
    case "bash":
    case "exec":
      return { icon: TerminalSquare, iconClass: "text-accent" };
    case "grep":
      return { icon: Search, iconClass: "text-status-info" };
    case "glob":
    case "list_files":
      return { icon: FolderTree, iconClass: "text-status-info" };
    case "fetch":
    case "web_fetch":
    case "web_search":
      return { icon: Globe, iconClass: "text-status-info" };
    case "spawn_subagent":
    case "task":
      return { icon: Bot, iconClass: "text-accent" };
    case "kb_search":
    case "kb_get_chunk":
      return { icon: BookOpen, iconClass: "text-status-info" };
    default:
      return { icon: Wrench, iconClass: "text-accent" };
  }
}

function toolLabel(name: string): string {
  switch (name) {
    case "bash":
    case "exec": return "命令";
    case "read_file":
    case "read": return "读取";
    case "write_file":
    case "write": return "写入";
    case "edit_file":
    case "edit": return "编辑";
    case "grep": return "搜索";
    case "glob":
    case "list_files": return "文件";
    case "web_fetch":
    case "fetch":
    case "web_search": return "网络";
    case "delegate_tasks":
    case "spawn_subagent":
    case "task": return "委派";
    case "kb_search":
    case "kb_get_chunk": return "知识";
    default: return name;
  }
}

// ── One-line summary of tool arguments (for collapsed view) ─────────────────
function summarizeArgs(name: string, raw: string): string | null {
  try {
    const args = JSON.parse(raw);
    switch (name) {
      case "read_file":
      case "read":
        if (args.offset != null || args.limit != null) {
          const start = args.offset ?? 1;
          const end = args.limit ? start + args.limit - 1 : "…";
          return `${args.path} (${start}-${end})`;
        }
        return args.path ?? null;
      case "write_file":
      case "write":
        return args.path ?? null;
      case "edit_file":
      case "edit": {
        const len = (args.old_string ?? "").length;
        return args.path ? `${args.path} (${len}b → ${(args.new_string ?? "").length}b)` : null;
      }
      case "bash":
      case "exec":
        return args.command ?? null;
      case "grep":
        return args.pattern ? `"${args.pattern}"${args.path ? ` in ${args.path}` : ""}` : null;
      case "glob":
      case "list_files":
        return args.pattern ?? args.path ?? null;
      case "fetch":
      case "web_fetch":
        return args.url ?? null;
      case "web_search":
        return args.query ?? null;
      case "spawn_subagent":
      case "task":
        return args.title ?? args.description ?? null;
      default:
        // Generic fallback: try common path/command/query fields
        return args.path ?? args.command ?? args.query ?? args.url ?? args.title ?? null;
    }
  } catch {
    return null;
  }
}

// Heuristic — mirrors src-tauri/src/tools/test_path.rs. Used by ToolCallCard
// to visually flag write/edit on test files so the user can immediately
// spot AI touching their tests and verify the justification.
function isTestPathFromArgs(toolName: string, raw: string): boolean {
  if (toolName !== "write_file" && toolName !== "edit_file" &&
      toolName !== "write" && toolName !== "edit") {
    return false;
  }
  try {
    const args = JSON.parse(raw);
    const path: string | undefined = args.path;
    if (!path) return false;
    const p = path.replace(/\\/g, "/").toLowerCase();
    if (/(^|\/)(tests?|__tests__|specs?)\//.test(p)) return true;
    if (/\.(test|spec)\.(ts|tsx|js|jsx|mjs)$/.test(p)) return true;
    if (/(^|\/)test_[^/]+\.py$/.test(p)) return true;
    if (/_test\.(py|go)$/.test(p)) return true;
    if (/_spec\.rb$/.test(p)) return true;
    if (/(test|tests)\.java$/.test(p)) return true;
    return false;
  } catch {
    return false;
  }
}

/** First non-empty line of a tool result, capped for the collapsed-card
 *  error summary. */
function firstNonEmptyLine(raw: string): string {
  let cursor = 0;
  while (cursor < raw.length) {
    const code = raw.charCodeAt(cursor);
    if (code === 10) {
      cursor += 1;
      continue;
    }
    if (code === 13 || code === 32 || code === 9) {
      cursor += 1;
      continue;
    }

    // Inspect at most the visible summary plus one character. `split`/`trim`
    // would allocate proportional to a multi-megabyte failed tool result even
    // while its card is collapsed.
    const probeEnd = Math.min(raw.length, cursor + 201);
    const newline = raw.indexOf("\n", cursor);
    const lineEnd =
      newline >= 0 && newline < probeEnd ? newline : probeEnd;
    let visibleEnd = Math.min(lineEnd, cursor + 200);
    while (visibleEnd > cursor) {
      const trailing = raw.charCodeAt(visibleEnd - 1);
      if (trailing !== 13 && trailing !== 32 && trailing !== 9) break;
      visibleEnd -= 1;
    }
    const truncated = lineEnd > cursor + 200 ||
      (newline < 0 && raw.length > cursor + 200);
    return `${raw.slice(cursor, visibleEnd)}${truncated ? "…" : ""}`;
  }
  return "";
}

function basename(path: string | undefined): string {
  if (!path) return "unknown source";
  const normalized = path.replace(/\\/g, "/");
  return normalized.split("/").filter(Boolean).pop() ?? path;
}

// Tools whose `path` argument names a file the agent just produced — once the
// call succeeds we offer to open it with the OS default app, so a generated
// deck / doc / sheet isn't just a path string the user has to hunt down.
const FILE_WRITE_TOOLS = new Set([
  "write_file", "write", "edit_file", "edit",
  "write_pptx", "edit_pptx", "format_pptx", "write_docx", "edit_xlsx",
]);

// A tool's `path` is usually relative to the session cwd; the OS opener needs
// an absolute path. Resolve it, handling both unix and Windows separators.
function resolveAgainstCwd(cwd: string | undefined, p: string): string {
  const isAbsolute = /^(\/|\\\\|[A-Za-z]:[\\/])/.test(p);
  if (isAbsolute || !cwd) return p;
  const sep = cwd.includes("\\") ? "\\" : "/";
  return `${cwd.replace(/[\\/]+$/, "")}${sep}${p.replace(/^[\\/]+/, "")}`;
}

// The file path to offer "open" for, or null if this isn't a successful
// file-writing call.
function generatedFilePath(tc: ToolCallState): string | null {
  if (!FILE_WRITE_TOOLS.has(tc.name)) return null;
  if (tc.status !== "done" || tc.isError) return null;
  try {
    const p = (JSON.parse(tc.args ?? "{}") as { path?: string }).path;
    return typeof p === "string" && p.trim() ? p : null;
  } catch {
    return null;
  }
}

function sourceDisplayName(source: KnowledgeSource): string {
  if (source.path) return basename(source.path);
  return source.title ?? source.document_id ?? source.chunk_id ?? "unknown source";
}

function parseKnowledgeSources(toolName: string, raw: string | null | undefined): KnowledgeSource[] {
  if (!raw || (toolName !== "kb_search" && toolName !== "kb_get_chunk")) return [];
  try {
    const parsed = JSON.parse(raw);
    const candidates: unknown[] = Array.isArray(parsed)
      ? parsed
      : Array.isArray(parsed?.results)
        ? parsed.results
        : [parsed];
    return candidates.filter((item: unknown): item is KnowledgeSource => {
      if (!item || typeof item !== "object") return false;
      const source = item as Partial<KnowledgeSource>;
      return Boolean(source.path || source.chunk_id || source.document_id);
    });
  } catch {
    return [];
  }
}

function sourceLocator(source: KnowledgeSource): string | null {
  if (source.slide != null) return `slide ${source.slide}`;
  if (source.page != null) return `page ${source.page}`;
  return null;
}

function KnowledgeSourcesList({ sources }: { sources: KnowledgeSource[] }) {
  return (
    <div className="space-y-2 font-sans">
      <div className="text-gray-500 text-[11px] font-medium">sources {sources.length}</div>
      <div className="space-y-1.5">
        {sources.map((source, i) => (
          <div key={`${source.chunk_id ?? source.document_id ?? source.path ?? "source"}-${i}`} className="rounded border border-border bg-surface-1 px-2.5 py-2">
            <div className="flex items-center gap-2 min-w-0">
              <BookOpen size={12} className="text-status-info shrink-0" />
              <span className="text-xs text-gray-200 font-medium truncate" title={source.path}>
                {sourceDisplayName(source)}
              </span>
              {source.kind && (
                <span className="text-[11px] uppercase text-gray-600 shrink-0">{source.kind}</span>
              )}
            </div>
            <div className="mt-1 flex flex-wrap items-center gap-1.5 text-[11px] text-gray-500 font-mono">
              {sourceLocator(source) && <span>{sourceLocator(source)}</span>}
              {source.chunk_id && <span>{source.chunk_id}</span>}
              {source.score != null && <span>score {source.score}</span>}
            </div>
            {(source.heading || source.title) && (
              <div className="mt-1 text-[11px] text-gray-400 truncate">{source.heading || source.title}</div>
            )}
            {source.snippet && (
              <div className="mt-1 text-[11px] text-gray-400 leading-relaxed whitespace-pre-wrap break-words">
                {source.snippet}
              </div>
            )}
            {source.path && (
              <div className="mt-1 text-[11px] text-gray-600 font-mono truncate" title={source.path}>
                {source.path}
              </div>
            )}
          </div>
        ))}
      </div>
    </div>
  );
}

export const ToolCallCard = memo(function ToolCallCard({ tc }: Props) {
  const [open, setOpen] = useState(false);
  const parsedDiff = useMemo(
    () => (open && tc.result != null ? parseUnifiedDiffResult(tc.result) : null),
    [open, tc.result],
  );
  const hasDiff = (parsedDiff?.files.length ?? 0) > 0;

  const { icon: Icon, iconClass } = styleForTool(tc.name);
  const summary = summarizeArgs(tc.name, tc.args ?? "");
  const isTestMod = isTestPathFromArgs(tc.name, tc.args ?? "");
  const knowledgeSources = useMemo(
    () => (open ? parseKnowledgeSources(tc.name, tc.result) : []),
    [open, tc.name, tc.result],
  );
  const cwd = useChatStore((s) => s.activeSession?.cwd);
  const filePath = generatedFilePath(tc);

  const statusIcon =
    tc.status === "waiting_permission" ? (
      <ShieldQuestion size={12} className="text-status-warning shrink-0" />
    ) : tc.status === "done" && !tc.isError ? (
      <CheckCircle size={12} className="text-status-success shrink-0" />
    ) : tc.status === "cancelled" ? (
      <Ban size={12} className="text-gray-500 shrink-0" aria-label="已取消" />
    ) : tc.status === "blocked" ? (
      <AlertCircle size={12} className="text-status-warning shrink-0" aria-label="已阻断" />
    ) : tc.status === "error" || tc.status === "denied" || tc.isError ? (
      <AlertCircle size={12} className="text-status-danger shrink-0" />
    ) : (
      <span className="w-3 h-3 rounded-full border border-accent animate-pulse motion-reduce:animate-none shrink-0" />
    );

  const needsAttention = tc.status !== "done" || Boolean(tc.isError);
  const shellClass = needsAttention
    ? tc.status === "error" || tc.status === "denied" || tc.isError
      ? "rounded-r-sm border-l border-status-danger/50 bg-transparent"
      : tc.status === "blocked"
        ? "rounded-r-md border-l-2 border-status-warning/50 bg-status-warning-soft/55"
      : tc.status === "waiting_permission"
        ? "rounded-r-md border-l-2 border-status-warning/40 bg-status-warning-soft/45"
        : "rounded-r-md border-l-2 border-accent/35 bg-accent/[0.025]"
    : "rounded-md";

  return (
    <div className={`my-0.5 w-fit max-w-full text-[13px] leading-5 ${shellClass}`} data-tool-status={tc.status}>
      <button
        data-density={needsAttention ? "attention" : "compact"}
        aria-label={`${toolLabel(tc.name)}${summary ? ` · ${summary}` : ""}`}
        className={`inline-flex min-h-7 max-w-full items-center gap-1.5 rounded-md px-1.5 text-left transition-colors hover:bg-surface-3/55 ${
          needsAttention ? "py-0.5" : "py-0"
        }`}
        onClick={() => setOpen((o) => !o)}
      >
        {open ? <ChevronDown size={12} className="text-gray-600 shrink-0" /> : <ChevronRight size={12} className="text-gray-600 shrink-0" />}
        <Icon size={12} className={`${needsAttention ? iconClass : "text-gray-600"} shrink-0`} />
        <span className={`shrink-0 ${needsAttention ? "text-gray-300" : "text-gray-500"}`}>{toolLabel(tc.name)}</span>
        {isTestMod && (
          <span className="shrink-0 rounded bg-surface-3 px-1 py-0.5 text-[11px] font-medium text-gray-500">
            test
          </span>
        )}
        {summary && (
          <span className="min-w-0 truncate font-mono text-[13px] text-gray-600">· {summary}</span>
        )}
        <span className="ml-auto shrink-0">{statusIcon}</span>
      </button>

      {/* A failed call must explain itself without a click: surface the
          first line of the error on the collapsed card. Full output stays
          behind the expand toggle. */}
      {!open && (tc.isError || tc.status === "error") && tc.result && (
        <div className="ml-7 max-w-[56ch] truncate px-1.5 pb-1 text-[13px] font-mono leading-5 text-status-danger">
          {firstNonEmptyLine(tc.result)}
        </div>
      )}
      {!open && tc.status === "blocked" && tc.result && (
        <div className="ml-7 max-w-[56ch] truncate px-1.5 pb-1 text-[13px] leading-5 text-status-warning">
          {firstNonEmptyLine(tc.result)}
        </div>
      )}

      {filePath && (
        <button
          onClick={() =>
            void invoke("plugin:shell|open", {
              path: resolveAgainstCwd(cwd, filePath),
            }).catch(() => {})
          }
          title={`打开 ${filePath}`}
          className="ml-7 flex max-w-[calc(100%-1.75rem)] items-center gap-1.5 rounded px-1.5 py-0.5 text-left text-[12px] text-accent transition-colors hover:bg-surface-3"
        >
          <ExternalLink size={11} className="shrink-0" />
          <span className="truncate font-mono">{basename(filePath)}</span>
          <span className="ml-auto shrink-0 text-[11px] text-gray-500">打开文件</span>
        </button>
      )}

      {open && (
        <div className="ml-2 border-l border-border/45 px-2.5 py-1.5 space-y-1.5 font-mono">
          {tc.args && (
            <div>
              <div className="text-gray-500 mb-1">input</div>
              <pre className="text-gray-300 whitespace-pre-wrap break-all">{formatArgs(tc.args)}</pre>
            </div>
          )}
          {tc.result != null && (
            <div>
              <div className={`mb-1 ${tc.isError ? "text-status-danger" : "text-gray-500"}`}>
                {tc.isError ? "error" : "output"}
              </div>
              {knowledgeSources.length > 0 && !tc.isError ? (
                <KnowledgeSourcesList sources={knowledgeSources} />
              ) : hasDiff ? (
                <DiffViewer output={tc.result} parsed={parsedDiff ?? undefined} />
              ) : (
                <pre className={`whitespace-pre-wrap break-all ${tc.isError ? "text-status-danger" : "text-gray-300"}`}>
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
});

function formatArgs(raw: string): string {
  try {
    return JSON.stringify(JSON.parse(raw), null, 2);
  } catch {
    return raw;
  }
}
