// SPDX-License-Identifier: Apache-2.0
import { useEffect, useState } from "react";
import { Copy, ExternalLink, FileText, X } from "lucide-react";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import { invoke } from "../lib/tauri";
import { FileArtifactCard } from "./FileArtifactCard";

interface DocumentPreviewData {
  path: string;
  relative_path: string;
  name: string;
  extension: string;
  content: string;
  truncated: boolean;
}

export interface DocumentTab {
  id: string;
  path: string;
  title: string;
}

interface Props {
  tab: DocumentTab;
  cwd?: string | null;
  onClose: () => void;
}

export function DocumentPreview({ tab, cwd, onClose }: Props) {
  const [preview, setPreview] = useState<DocumentPreviewData | null>(null);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    let cancelled = false;
    setPreview(null);
    setError(null);
    void invoke<DocumentPreviewData>("read_document", { cwd: cwd ?? "", path: tab.path })
      .then((value) => { if (!cancelled) setPreview(value); })
      .catch((reason) => { if (!cancelled) setError(String(reason)); });
    return () => { cancelled = true; };
  }, [cwd, tab.path]);

  const isMarkdown = preview?.extension === "md" || preview?.extension === "markdown";
  return (
    <section className="flex min-h-0 flex-1 flex-col bg-surface-0" aria-label={`文档预览：${tab.title}`}>
      <header className="flex shrink-0 items-center gap-2 border-b border-border px-3 py-2">
        <FileText size={14} className="shrink-0 text-status-info" aria-hidden="true" />
        <div className="min-w-0 flex-1">
          <div className="truncate text-xs font-medium text-gray-200">{tab.title}</div>
          <div className="truncate font-mono text-[11px] text-gray-500">{preview?.relative_path ?? tab.path}</div>
        </div>
        <button type="button" className="rounded p-1 text-gray-500 hover:bg-surface-3 hover:text-gray-200" onClick={() => void navigator.clipboard?.writeText(preview?.path ?? tab.path)} aria-label={`复制路径 ${tab.title}`}>
          <Copy size={12} />
        </button>
        <button type="button" className="rounded p-1 text-gray-500 hover:bg-surface-3 hover:text-gray-200" onClick={() => void invoke("plugin:shell|open", { path: preview?.path ?? tab.path }).catch(() => {})} aria-label={`系统打开 ${tab.title}`}>
          <ExternalLink size={12} />
        </button>
        <button type="button" className="rounded p-1 text-gray-500 hover:bg-surface-3 hover:text-gray-200" onClick={onClose} aria-label={`关闭文档 ${tab.title}`}>
          <X size={13} />
        </button>
      </header>
      {error ? (
        <div className="m-4 rounded-lg border border-status-danger/30 bg-status-danger/5 p-4 text-sm text-status-danger">
          <div className="font-medium">文档无法预览</div>
          <div className="mt-1 break-words text-xs">{error}</div>
          <FileArtifactCard path={tab.path} cwd={cwd} compact />
        </div>
      ) : !preview ? (
        <div className="flex flex-1 items-center justify-center text-xs text-gray-500">正在读取文档…</div>
      ) : (
        <div className="min-h-0 flex-1 overflow-y-auto p-5">
          {preview.truncated && <div className="mb-3 rounded border border-status-warning/30 bg-status-warning-soft/30 px-3 py-2 text-xs text-status-warning">文档较大，仅显示前 512 KB。</div>}
          {isMarkdown ? (
            <article className="prose prose-sm max-w-none dark:prose-invert [&>*:first-child]:mt-0">
              <ReactMarkdown remarkPlugins={[remarkGfm]}>{preview.content}</ReactMarkdown>
            </article>
          ) : (
            <pre className="whitespace-pre-wrap break-words font-mono text-xs leading-6 text-gray-300">{preview.content}</pre>
          )}
        </div>
      )}
    </section>
  );
}
