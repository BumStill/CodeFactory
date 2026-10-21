// SPDX-License-Identifier: Apache-2.0
// File-artifact card as rendered inside the message stream. It shares the
// legacy card's extension rules but routes every open through the controlled
// native link command, so a missing file surfaces a toast instead of nothing.
import { Copy, ExternalLink, FileText, Eye } from "lucide-react";
import { useContext } from "react";
import { FileLinkScope, openFileLinkVisible, resolveLocalPath } from "./MessageFileLinks";

export { isDocumentPath } from "./FileArtifactCard";

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
  sessionId?: string | null;
  onPreview?: (path: string) => void;
  compact?: boolean;
}

export function MessageFileArtifactCard({ path, cwd, sessionId, onPreview, compact = false }: Props) {
  const scope = useContext(FileLinkScope);
  cwd = cwd ?? scope.cwd;
  sessionId = sessionId ?? scope.sessionId;
  const resolved = resolveLocalPath(cwd, path);
  const name = basename(path);
  const handleOpen = () => {
    openFileLinkVisible(path, cwd, sessionId);
  };
  const handleCopy = () => {
    void navigator.clipboard?.writeText(resolved);
  };
  return (
    <span className={`my-2 inline-flex max-w-full items-center gap-2 rounded-lg border border-border bg-surface-1 align-middle ${compact ? "px-2 py-1" : "px-2.5 py-2"}`}>
      <FileText size={compact ? 12 : 14} className="shrink-0 text-status-info" aria-hidden="true" />
      <span className="min-w-0">
        <span className="flex items-center gap-1.5 text-label font-medium text-gray-200">
          <span className="truncate" title={resolved}>{name}</span>
          <span className="shrink-0 rounded bg-surface-3 px-1 text-caption text-gray-500">{extension(path)}</span>
        </span>
        {!compact && <span className="block max-w-[32ch] truncate font-mono text-caption text-gray-500" title={resolved}>{path}</span>}
      </span>
      {onPreview && (
        <button type="button" className="inline-flex shrink-0 items-center gap-1 rounded px-1.5 py-1 text-caption text-accent hover:bg-surface-3" onClick={() => onPreview(path)} aria-label={`查看文档 ${name}`}>
          <Eye size={14} /> 查看
        </button>
      )}
      <button type="button" className="inline-flex shrink-0 items-center gap-1 rounded px-1.5 py-1 text-caption text-gray-500 hover:bg-surface-3 hover:text-gray-200" onClick={handleOpen} aria-label={`系统打开 ${name}`} title="用系统应用打开">
        <ExternalLink size={14} /> 打开
      </button>
      <button type="button" className="rounded p-1 text-gray-500 hover:bg-surface-3 hover:text-gray-200" onClick={handleCopy} aria-label={`复制路径 ${name}`} title="复制完整路径">
        <Copy size={14} />
      </button>
    </span>
  );
}
