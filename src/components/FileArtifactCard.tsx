// SPDX-License-Identifier: Apache-2.0
import { Copy, ExternalLink, FileText, Eye } from "lucide-react";
import { invoke } from "../lib/tauri";

const TEXT_EXTENSIONS = new Set([
  "md", "markdown", "txt", "json", "yaml", "yml", "toml", "csv",
  "ts", "tsx", "js", "jsx", "rs", "py", "go", "java", "c", "cpp",
  "h", "hpp", "css", "scss", "html", "sql", "sh", "bash", "xml",
]);

export function resolveFilePath(cwd: string | null | undefined, path: string): string {
  const raw = path.startsWith("file://") ? path.slice("file://".length) : path;
  if (/^(\/|\\\\|[A-Za-z]:[\\/])/.test(raw) || !cwd) return raw;
  const separator = cwd.includes("\\") ? "\\" : "/";
  return `${cwd.replace(/[\\/]+$/, "")}${separator}${raw.replace(/^[\\/]+/, "")}`;
}

export function isDocumentPath(value: string): boolean {
  const path = value.trim().replace(/^file:\/\//, "");
  if (!path || /^(https?:|mailto:)/i.test(path) || /[(){}=<>]/.test(path) || /\s/.test(path) && !path.includes("/")) return false;
  if (!path.includes("/") && !/^[A-Za-z]:[\\/]/.test(path)) return false;
  const name = path.replace(/\\/g, "/").split("/").pop() ?? "";
  const extension = name.includes(".") ? name.split(".").pop()?.toLowerCase() : "";
  return Boolean(extension && (TEXT_EXTENSIONS.has(extension) || ["pdf", "docx", "pptx", "xlsx"].includes(extension)));
}

function basename(path: string): string {
  return path.replace(/\\/g, "/").split("/").filter(Boolean).pop() ?? path;
}

function extension(path: string): string {
  const name = basename(path);
  return name.includes(".") ? name.split(".").pop()?.toUpperCase() ?? "FILE" : "FILE";
}

interface Props {
  path: string;
  cwd?: string | null;
  onPreview?: (path: string) => void;
  compact?: boolean;
}

export function FileArtifactCard({ path, cwd, onPreview, compact = false }: Props) {
  const resolved = resolveFilePath(cwd, path);
  const name = basename(path);
  const handleOpen = () => {
    void invoke("plugin:shell|open", { path: resolved }).catch(() => {});
  };
  const handleCopy = () => {
    void navigator.clipboard?.writeText(resolved);
  };
  return (
    <span className={`my-2 inline-flex max-w-full items-center gap-2 rounded-lg border border-border bg-surface-1 align-middle ${compact ? "px-2 py-1" : "px-2.5 py-2"}`}>
      <FileText size={compact ? 12 : 14} className="shrink-0 text-status-info" aria-hidden="true" />
      <span className="min-w-0">
        <span className="flex items-center gap-1.5 text-xs font-medium text-gray-200">
          <span className="truncate" title={resolved}>{name}</span>
          <span className="shrink-0 rounded bg-surface-3 px-1 text-[10px] text-gray-500">{extension(path)}</span>
        </span>
        {!compact && <span className="block max-w-[32ch] truncate font-mono text-[11px] text-gray-500" title={resolved}>{path}</span>}
      </span>
      {onPreview && (
        <button type="button" className="inline-flex shrink-0 items-center gap-1 rounded px-1.5 py-1 text-[11px] text-accent hover:bg-surface-3" onClick={() => onPreview(path)} aria-label={`查看文档 ${name}`}>
          <Eye size={11} /> 查看
        </button>
      )}
      <button type="button" className="inline-flex shrink-0 items-center gap-1 rounded px-1.5 py-1 text-[11px] text-gray-500 hover:bg-surface-3 hover:text-gray-200" onClick={handleOpen} aria-label={`系统打开 ${name}`} title="用系统应用打开">
        <ExternalLink size={11} /> 打开
      </button>
      <button type="button" className="rounded p-1 text-gray-500 hover:bg-surface-3 hover:text-gray-200" onClick={handleCopy} aria-label={`复制路径 ${name}`} title="复制完整路径">
        <Copy size={11} />
      </button>
    </span>
  );
}
