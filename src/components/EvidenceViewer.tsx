// SPDX-License-Identifier: Apache-2.0
//! Evidence Pack viewer — Phase 6.
//! Full-panel overlay showing all evidence pack tabs.

import { useEffect, useState } from "react";
import {
  X,
  FolderOpen,
  CheckCircle,
  XCircle,
  Clock,
  Coins,
  FileText,
  Terminal,
  GitCommit,
  Bot,
  Filter,
  AlertCircle,
  BookOpen,
} from "lucide-react";
import { invoke } from "../lib/tauri";
import { DiffViewer } from "./DiffViewer";

// ── Types ─────────────────────────────────────────────────────────────────────

export interface EvidencePackMeta {
  spec_req_id: string;
  spec_title: string;
  task_run_ids: string[];
  session_id: string;
  created_at: string;
  completed_at: string;
  status: "passed" | "failed" | "partial";
  total_tasks: number;
  completed_tasks: number;
  failed_tasks: number;
  total_tool_calls: number;
  files_changed: number;
  verification_passed: boolean;
  total_tokens: number;
  duration_minutes: number;
  path: string;
}

export interface EvidencePack {
  manifest: EvidencePackMeta;
  summary_md: string;
  tool_calls: ToolCallEntry[];
  knowledge_refs?: KnowledgeRefEntry[];
  files_changed: FileChangedEntry[];
  verification: VerificationEntry[];
  git_commits: GitCommitEntry[];
  ai_collaboration: AiCollaboration;
}

interface ToolCallEntry {
  tool_name: string;
  args: Record<string, unknown>;
  result_preview: string;
  timestamp: string;
  task_id?: string;
  duration_ms?: number;
}

interface KnowledgeRefEntry {
  id: string;
  session_id?: string | null;
  task_id?: string | null;
  query: string;
  filters?: Record<string, unknown>;
  result_refs: KnowledgeRefSource[];
  created_at: string;
  latency_ms: number;
}

interface KnowledgeRefSource {
  chunk_id?: string;
  document_id?: string;
  path?: string;
  page?: number | null;
  slide?: number | null;
}

interface FileChangedEntry {
  path: string;
  diff: string;
}

interface VerificationEntry {
  check: string;
  passed: boolean;
  output: string;
  duration_ms: number;
  task_id?: string;
  task_title?: string;
}

interface GitCommitEntry {
  hash: string;
  short_hash: string;
  message: string;
  author: string;
  email: string;
  timestamp: string;
}

interface AiCollaboration {
  model: string;
  total_tokens: number;
  assumptions: string[];
  review_points: string[];
}

// ── Helpers ───────────────────────────────────────────────────────────────────

function StatusBadge({ status }: { status: "passed" | "failed" | "partial" }) {
  const map = {
    passed: "bg-green-800 text-green-200",
    failed: "bg-red-800 text-red-200",
    partial: "bg-yellow-800 text-yellow-200",
  };
  return (
    <span className={`px-2 py-0.5 rounded text-[10px] font-semibold uppercase ${map[status]}`}>
      {status}
    </span>
  );
}

function formatDuration(minutes: number): string {
  if (minutes < 1) return `${Math.round(minutes * 60)}s`;
  if (minutes < 60) return `${minutes.toFixed(1)}m`;
  return `${(minutes / 60).toFixed(1)}h`;
}

function formatTokens(n: number): string {
  if (n >= 1000) return `${(n / 1000).toFixed(1)}k`;
  return String(n);
}

function formatTs(ts: string): string {
  if (!ts) return "—";
  try {
    return new Date(ts).toLocaleString();
  } catch {
    return ts;
  }
}

function sourceFileName(path: string | undefined): string {
  if (!path) return "unknown source";
  const normalized = path.replace(/\\/g, "/");
  return normalized.split("/").filter(Boolean).pop() ?? path;
}

function sourceLocator(source: KnowledgeRefSource): string | null {
  if (source.slide != null) return `slide ${source.slide}`;
  if (source.page != null) return `page ${source.page}`;
  return null;
}

// ── Simple markdown renderer (same logic as SpecsPage) ───────────────────────

function escapeHtml(s: string): string {
  return s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
}

function renderInline(text: string): string {
  text = text.replace(/\*\*(.+?)\*\*/g, "<strong>$1</strong>");
  text = text.replace(/\*(.+?)\*/g, "<em>$1</em>");
  text = text.replace(/`([^`]+)`/g, '<code class="inline-code">$1</code>');
  return text;
}

function renderMarkdown(raw: string): string {
  const lines = raw.split("\n");
  const out: string[] = [];
  let inCodeBlock = false;
  let inList = false;

  const closeList = () => {
    if (inList) { out.push("</ul>"); inList = false; }
  };

  for (const line of lines) {
    if (line.startsWith("```")) {
      if (inCodeBlock) { out.push("</code></pre>"); inCodeBlock = false; }
      else { closeList(); out.push(`<pre><code class="code-block">`); inCodeBlock = true; }
      continue;
    }
    if (inCodeBlock) { out.push(escapeHtml(line)); continue; }

    const hm = line.match(/^(#{1,6})\s+(.*)/);
    if (hm) {
      closeList();
      const lv = hm[1].length;
      out.push(`<h${lv}>${renderInline(escapeHtml(hm[2]))}</h${lv}>`);
      continue;
    }
    const ulm = line.match(/^[-*]\s+(.*)/);
    if (ulm) {
      if (!inList) { out.push("<ul>"); inList = true; }
      out.push(`<li>${renderInline(escapeHtml(ulm[1]))}</li>`);
      continue;
    }
    if (line.trim() === "") { closeList(); out.push("<p></p>"); continue; }
    closeList();
    out.push(`<p>${renderInline(escapeHtml(line))}</p>`);
  }
  closeList();
  if (inCodeBlock) out.push("</code></pre>");
  return out.join("\n");
}

// ── Tab components ────────────────────────────────────────────────────────────

function SummaryTab({ md }: { md: string }) {
  return (
    <div className="flex-1 overflow-y-auto p-6">
      <div
        className="spec-preview max-w-3xl"
        dangerouslySetInnerHTML={{ __html: renderMarkdown(md) }}
      />
    </div>
  );
}

function ToolCallsTab({ toolCalls }: { toolCalls: ToolCallEntry[] }) {
  const [filter, setFilter] = useState("");
  const tools = [...new Set(toolCalls.map((t) => t.tool_name))].sort();
  const filtered = filter
    ? toolCalls.filter((t) => t.tool_name === filter)
    : toolCalls;

  return (
    <div className="flex flex-col flex-1 min-h-0">
      <div className="flex items-center gap-2 px-4 py-2 border-b border-border shrink-0">
        <Filter size={12} className="text-gray-500" />
        <select
          value={filter}
          onChange={(e) => setFilter(e.target.value)}
          className="bg-surface-3 border border-border rounded px-2 py-1 text-xs text-gray-300 outline-none"
        >
          <option value="">All tools ({toolCalls.length})</option>
          {tools.map((t) => (
            <option key={t} value={t}>
              {t} ({toolCalls.filter((c) => c.tool_name === t).length})
            </option>
          ))}
        </select>
      </div>
      <div className="flex-1 overflow-y-auto">
        <table className="w-full text-xs">
          <thead className="sticky top-0 bg-surface-2">
            <tr className="text-left text-gray-500 border-b border-border">
              <th className="px-3 py-2 font-medium">Time</th>
              <th className="px-3 py-2 font-medium">Tool</th>
              <th className="px-3 py-2 font-medium">Args</th>
              <th className="px-3 py-2 font-medium">Result</th>
            </tr>
          </thead>
          <tbody>
            {filtered.map((tc, i) => (
              <tr key={i} className="border-b border-border/50 hover:bg-surface-2/50">
                <td className="px-3 py-1.5 text-gray-500 whitespace-nowrap font-mono">
                  {tc.timestamp ? new Date(tc.timestamp).toLocaleTimeString() : "—"}
                </td>
                <td className="px-3 py-1.5 text-accent font-mono whitespace-nowrap">
                  {tc.tool_name}
                </td>
                <td className="px-3 py-1.5 text-gray-400 font-mono max-w-xs truncate">
                  {JSON.stringify(tc.args).slice(0, 100)}
                </td>
                <td className="px-3 py-1.5 text-gray-500 max-w-xs truncate">
                  {tc.result_preview || "—"}
                </td>
              </tr>
            ))}
          </tbody>
        </table>
        {filtered.length === 0 && (
          <div className="text-center text-gray-600 text-xs py-8">No tool calls recorded</div>
        )}
      </div>
    </div>
  );
}

function FilesChangedTab({ files }: { files: FileChangedEntry[] }) {
  const [expanded, setExpanded] = useState<number | null>(0);

  if (files.length === 0) {
    return (
      <div className="flex-1 flex items-center justify-center text-gray-600 text-xs">
        No files changed recorded
      </div>
    );
  }

  return (
    <div className="flex-1 overflow-y-auto p-4 space-y-2">
      {files.map((f, i) => (
        <div key={i} className="border border-border rounded overflow-hidden">
          <button
            onClick={() => setExpanded(expanded === i ? null : i)}
            className="w-full flex items-center gap-2 px-3 py-2 bg-surface-2 text-left hover:bg-surface-3 transition-colors"
          >
            <FileText size={12} className="text-gray-500 shrink-0" />
            <span className="flex-1 text-xs font-mono text-gray-300 truncate">{f.path}</span>
            <span className="text-gray-600 text-[10px]">{expanded === i ? "▲" : "▼"}</span>
          </button>
          {expanded === i && f.diff && (
            <div className="p-2">
              <DiffViewer output={f.diff} />
            </div>
          )}
          {expanded === i && !f.diff && (
            <div className="px-3 py-2 text-xs text-gray-600 italic">No diff available</div>
          )}
        </div>
      ))}
    </div>
  );
}

function VerificationTab({ items }: { items: VerificationEntry[] }) {
  if (items.length === 0) {
    return (
      <div className="flex-1 flex items-center justify-center text-gray-600 text-xs">
        No verification results recorded
      </div>
    );
  }

  return (
    <div className="flex-1 overflow-y-auto p-4 space-y-2">
      {items.map((v, i) => (
        <div key={i} className={`rounded border p-3 ${v.passed ? "border-green-800 bg-green-950/20" : "border-red-800 bg-red-950/20"}`}>
          <div className="flex items-center gap-2 mb-1">
            {v.passed
              ? <CheckCircle size={12} className="text-green-400 shrink-0" />
              : <XCircle size={12} className="text-red-400 shrink-0" />}
            <span className="text-xs font-medium text-gray-200">{v.check}</span>
            {v.task_title && (
              <span className="text-[10px] text-gray-500 ml-auto">task: {v.task_title}</span>
            )}
            <span className="text-[10px] text-gray-600">{v.duration_ms}ms</span>
          </div>
          {v.output && (
            <pre className="text-[11px] text-gray-400 whitespace-pre-wrap font-mono leading-relaxed mt-1 max-h-32 overflow-y-auto">
              {v.output}
            </pre>
          )}
        </div>
      ))}
    </div>
  );
}

function GitHistoryTab({ commits }: { commits: GitCommitEntry[] }) {
  if (commits.length === 0) {
    return (
      <div className="flex-1 flex items-center justify-center text-gray-600 text-xs">
        No commits recorded during this session
      </div>
    );
  }

  return (
    <div className="flex-1 overflow-y-auto p-4 space-y-1">
      {commits.map((c, i) => (
        <div key={i} className="flex items-start gap-3 p-2 rounded hover:bg-surface-2 transition-colors">
          <GitCommit size={14} className="text-gray-600 mt-0.5 shrink-0" />
          <div className="flex-1 min-w-0">
            <div className="flex items-center gap-2">
              <span className="font-mono text-[11px] text-accent">{c.short_hash}</span>
              <span className="text-xs text-gray-300 truncate">{c.message}</span>
            </div>
            <div className="flex items-center gap-2 text-[10px] text-gray-600 mt-0.5">
              <span>{c.author}</span>
              <span>·</span>
              <span>{formatTs(c.timestamp)}</span>
            </div>
          </div>
        </div>
      ))}
    </div>
  );
}

function AiCollabTab({ data }: { data: AiCollaboration | null }) {
  if (!data) {
    return (
      <div className="flex-1 flex items-center justify-center text-gray-600 text-xs">
        No AI collaboration data available
      </div>
    );
  }

  return (
    <div className="flex-1 overflow-y-auto p-4 space-y-4">
      <div className="grid grid-cols-2 gap-3">
        <div className="bg-surface-2 rounded p-3">
          <div className="text-[10px] text-gray-600 uppercase tracking-wider mb-1">Model</div>
          <div className="text-xs text-gray-200 font-mono">{data.model}</div>
        </div>
        <div className="bg-surface-2 rounded p-3">
          <div className="text-[10px] text-gray-600 uppercase tracking-wider mb-1">Total Tokens</div>
          <div className="text-xs text-gray-200 font-mono">{formatTokens(data.total_tokens)}</div>
        </div>
      </div>

      <div>
        <div className="text-[10px] text-gray-600 uppercase tracking-wider mb-2">Assumptions Made</div>
        {data.assumptions.length === 0 ? (
          <div className="text-xs text-gray-600 italic">None recorded</div>
        ) : (
          <ul className="space-y-1">
            {data.assumptions.map((a, i) => (
              <li key={i} className="flex gap-2 text-xs text-gray-400">
                <AlertCircle size={12} className="text-yellow-600 shrink-0 mt-0.5" />
                <span>{a}</span>
              </li>
            ))}
          </ul>
        )}
      </div>

      <div>
        <div className="text-[10px] text-gray-600 uppercase tracking-wider mb-2">Review Points</div>
        {data.review_points.length === 0 ? (
          <div className="text-xs text-gray-600 italic">None recorded</div>
        ) : (
          <ul className="space-y-1">
            {data.review_points.map((r, i) => (
              <li key={i} className="flex gap-2 text-xs text-gray-400">
                <XCircle size={12} className="text-red-600 shrink-0 mt-0.5" />
                <span>{r}</span>
              </li>
            ))}
          </ul>
        )}
      </div>
    </div>
  );
}

function SourcesTab({ refs }: { refs: KnowledgeRefEntry[] }) {
  if (refs.length === 0) {
    return (
      <div className="flex-1 flex items-center justify-center text-gray-600 text-xs">
        No knowledge sources recorded
      </div>
    );
  }

  return (
    <div className="flex-1 overflow-y-auto p-4 space-y-3">
      {refs.map((ref) => (
        <div key={ref.id} className="rounded border border-border bg-surface-2 p-3">
          <div className="flex items-start gap-2">
            <BookOpen size={14} className="text-emerald-400 shrink-0 mt-0.5" />
            <div className="flex-1 min-w-0">
              <div className="flex items-center gap-2 min-w-0">
                <span className="text-xs text-gray-200 font-medium truncate">{ref.query}</span>
                <span className="text-[10px] text-gray-600 shrink-0">{ref.latency_ms}ms</span>
              </div>
              <div className="mt-1 text-[10px] text-gray-600">
                {ref.result_refs.length} result{ref.result_refs.length === 1 ? "" : "s"}
              </div>
            </div>
          </div>
          <div className="mt-3 space-y-1.5">
            {ref.result_refs.map((source, i) => (
              <div key={`${source.chunk_id ?? source.document_id ?? source.path ?? "source"}-${i}`} className="rounded border border-border/70 bg-surface-1 px-2.5 py-2">
                <div className="flex items-center gap-2 min-w-0">
                  <FileText size={12} className="text-gray-500 shrink-0" />
                  <span className="text-xs text-gray-300 font-medium truncate" title={source.path}>
                    {sourceFileName(source.path)}
                  </span>
                </div>
                <div className="mt-1 flex flex-wrap items-center gap-1.5 text-[10px] text-gray-500 font-mono">
                  {sourceLocator(source) && <span>{sourceLocator(source)}</span>}
                  {source.chunk_id && <span>{source.chunk_id}</span>}
                  {source.document_id && <span>doc {source.document_id}</span>}
                </div>
                {source.path && (
                  <div className="mt-1 text-[10px] text-gray-600 font-mono truncate" title={source.path}>
                    {source.path}
                  </div>
                )}
              </div>
            ))}
          </div>
        </div>
      ))}
    </div>
  );
}

// ── Main EvidenceViewer ───────────────────────────────────────────────────────

type TabKey = "summary" | "tool_calls" | "sources" | "files" | "verification" | "git" | "ai";

interface EvidenceViewerProps {
  packPath: string;
  onClose: () => void;
}

export function EvidenceViewer({ packPath, onClose }: EvidenceViewerProps) {
  const [pack, setPack] = useState<EvidencePack | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [tab, setTab] = useState<TabKey>("summary");

  useEffect(() => {
    setLoading(true);
    setError(null);
    invoke<EvidencePack>("get_evidence_pack", { path: packPath })
      .then(setPack)
      .catch((e) => setError(String(e)))
      .finally(() => setLoading(false));
  }, [packPath]);

  const openFolder = () => {
    invoke("open_evidence_pack_dir", { path: packPath }).catch(() => {});
  };

  const tabs: { key: TabKey; label: string; icon: React.ReactNode }[] = [
    { key: "summary", label: "Summary", icon: <FileText size={12} /> },
    { key: "tool_calls", label: `Tool Calls${pack ? ` (${pack.tool_calls.length})` : ""}`, icon: <Terminal size={12} /> },
    { key: "sources", label: `Sources${pack ? ` (${pack.knowledge_refs?.length ?? 0})` : ""}`, icon: <BookOpen size={12} /> },
    { key: "files", label: `Files${pack ? ` (${pack.files_changed.length})` : ""}`, icon: <FileText size={12} /> },
    { key: "verification", label: "Verification", icon: <CheckCircle size={12} /> },
    { key: "git", label: `Git${pack ? ` (${pack.git_commits.length})` : ""}`, icon: <GitCommit size={12} /> },
    { key: "ai", label: "AI", icon: <Bot size={12} /> },
  ];

  return (
    <div className="fixed inset-y-0 right-0 z-40 flex flex-col w-full max-w-full sm:w-[720px] border-l border-border bg-surface-0 shadow-2xl">
      {/* Header */}
      <div className="flex items-center gap-3 px-4 py-3 border-b border-border bg-surface-1 shrink-0">
        <div className="flex-1 min-w-0">
          {pack ? (
            <div className="flex items-center gap-2 flex-wrap">
              <span className="text-xs font-mono text-accent">{pack.manifest.spec_req_id}</span>
              <span className="text-xs font-semibold text-gray-200 truncate">{pack.manifest.spec_title}</span>
              <StatusBadge status={pack.manifest.status} />
            </div>
          ) : (
            <span className="text-xs text-gray-500">Loading...</span>
          )}
          {pack && (
            <div className="flex items-center gap-3 mt-1 text-[10px] text-gray-500">
              <span className="flex items-center gap-1">
                <Clock size={10} />
                {formatDuration(pack.manifest.duration_minutes)}
              </span>
              <span className="flex items-center gap-1">
                <Coins size={10} />
                {formatTokens(pack.manifest.total_tokens)} tokens
              </span>
              <span>{pack.manifest.total_tasks} tasks · {pack.manifest.total_tool_calls} calls · {pack.manifest.files_changed} files</span>
            </div>
          )}
        </div>
        <button
          onClick={openFolder}
          className="flex items-center gap-1 px-2 py-1 rounded text-xs text-gray-500 hover:text-gray-300 hover:bg-surface-3 transition-colors shrink-0"
          title="Open in Explorer"
        >
          <FolderOpen size={12} />
          <span>Open Folder</span>
        </button>
        <button
          onClick={onClose}
          className="p-1 rounded text-gray-600 hover:text-gray-300 hover:bg-surface-3 transition-colors"
        >
          <X size={14} />
        </button>
      </div>

      {/* Tab bar */}
      <div className="flex border-b border-border bg-surface-1 shrink-0 overflow-x-auto">
        {tabs.map((t) => (
          <button
            key={t.key}
            onClick={() => setTab(t.key)}
            className={`flex items-center gap-1.5 px-3 py-2 text-xs whitespace-nowrap border-b-2 transition-colors ${
              tab === t.key
                ? "border-accent text-gray-200"
                : "border-transparent text-gray-500 hover:text-gray-300"
            }`}
          >
            {t.icon}
            {t.label}
          </button>
        ))}
      </div>

      {/* Content */}
      <div className="flex flex-1 flex-col min-h-0">
        {loading && (
          <div className="flex-1 flex items-center justify-center text-gray-600 text-xs">
            Loading evidence pack...
          </div>
        )}
        {error && (
          <div className="flex-1 flex items-center justify-center text-red-400 text-xs p-4">
            {error}
          </div>
        )}
        {!loading && !error && pack && (
          <>
            {tab === "summary" && <SummaryTab md={pack.summary_md} />}
            {tab === "tool_calls" && <ToolCallsTab toolCalls={pack.tool_calls} />}
            {tab === "sources" && <SourcesTab refs={pack.knowledge_refs ?? []} />}
            {tab === "files" && <FilesChangedTab files={pack.files_changed} />}
            {tab === "verification" && <VerificationTab items={pack.verification} />}
            {tab === "git" && <GitHistoryTab commits={pack.git_commits} />}
            {tab === "ai" && <AiCollabTab data={pack.ai_collaboration} />}
          </>
        )}
      </div>
    </div>
  );
}

// ── Evidence Pack List item ───────────────────────────────────────────────────

interface EvidencePackListProps {
  specReqId: string;
  cwd: string;
}

export function EvidencePackList({ specReqId, cwd }: EvidencePackListProps) {
  const [packs, setPacks] = useState<EvidencePackMeta[]>([]);
  const [loading, setLoading] = useState(true);
  const [viewerPath, setViewerPath] = useState<string | null>(null);

  useEffect(() => {
    if (!cwd) return;
    setLoading(true);
    invoke<EvidencePackMeta[]>("list_evidence_packs", { cwd })
      .then((all) => setPacks(all.filter((p) => p.spec_req_id === specReqId)))
      .catch(() => setPacks([]))
      .finally(() => setLoading(false));
  }, [cwd, specReqId]);

  if (loading) {
    return <div className="text-xs text-gray-600 p-3">Loading evidence packs...</div>;
  }

  if (packs.length === 0) {
    return (
      <div className="flex flex-col items-center justify-center py-8 gap-2 text-gray-600">
        <FileText size={24} className="opacity-30" />
        <div className="text-xs">No evidence packs yet for {specReqId}</div>
        <div className="text-[10px] text-gray-700">Run "Start Implementation" to generate one</div>
      </div>
    );
  }

  return (
    <>
      <div className="flex-1 overflow-y-auto p-2 space-y-1">
        {packs.map((p) => (
          <button
            key={p.path}
            onClick={() => setViewerPath(p.path)}
            className="w-full flex items-center gap-3 p-3 rounded border border-border bg-surface-2 hover:bg-surface-3 text-left transition-colors"
          >
            <div className="flex-1 min-w-0">
              <div className="flex items-center gap-2">
                <StatusBadge status={p.status} />
                <span className="text-xs text-gray-300">{new Date(p.created_at).toLocaleString()}</span>
              </div>
              <div className="flex items-center gap-3 mt-1 text-[10px] text-gray-500">
                <span>{p.total_tasks} tasks</span>
                <span>{p.total_tool_calls} tool calls</span>
                <span>{formatDuration(p.duration_minutes)}</span>
                <span>{formatTokens(p.total_tokens)} tokens</span>
              </div>
            </div>
            {p.verification_passed
              ? <CheckCircle size={14} className="text-green-500 shrink-0" />
              : <XCircle size={14} className="text-red-500 shrink-0" />}
          </button>
        ))}
      </div>

      {viewerPath && (
        <EvidenceViewer
          packPath={viewerPath}
          onClose={() => setViewerPath(null)}
        />
      )}
    </>
  );
}
