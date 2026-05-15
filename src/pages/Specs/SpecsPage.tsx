// SPDX-License-Identifier: Apache-2.0
import { useCallback, useEffect, useRef, useState } from "react";
import {
  Plus,
  Trash2,
  CheckCircle,
  Play,
  Sparkles,
  X,
  ChevronLeft,
  Archive,
  Download,
} from "lucide-react";
import { invoke } from "../../lib/tauri";
import { useSpecsStore, type SpecMeta } from "../../stores/specs";
import { useTasksStore } from "../../stores/tasks";
import { useChatStore } from "../../stores/chat";
import { EvidencePackList, EvidenceViewer } from "../../components/EvidenceViewer";
import { useGitRemoteStore } from "../../stores/gitRemote";

// ── Minimal markdown renderer ─────────────────────────────────────────────────

function escapeHtml(s: string): string {
  return s
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;");
}

function renderInline(text: string): string {
  // Bold: **text**
  text = text.replace(/\*\*(.+?)\*\*/g, "<strong>$1</strong>");
  // Italic: *text*
  text = text.replace(/\*(.+?)\*/g, "<em>$1</em>");
  // Inline code: `text`
  text = text.replace(/`([^`]+)`/g, '<code class="inline-code">$1</code>');
  return text;
}

function renderMarkdown(raw: string): string {
  const lines = raw.split("\n");
  const out: string[] = [];
  let inCodeBlock = false;
  let inTable = false;
  let inList = false;
  let listOrdered = false;

  const closeList = () => {
    if (inList) {
      out.push(listOrdered ? "</ol>" : "</ul>");
      inList = false;
    }
  };

  for (let i = 0; i < lines.length; i++) {
    const line = lines[i];

    // Code block toggle
    if (line.startsWith("```")) {
      if (inCodeBlock) {
        out.push("</code></pre>");
        inCodeBlock = false;
      } else {
        closeList();
        const lang = line.slice(3).trim();
        out.push(`<pre><code class="code-block" data-lang="${escapeHtml(lang)}">`);
        inCodeBlock = true;
      }
      continue;
    }

    if (inCodeBlock) {
      out.push(escapeHtml(line));
      continue;
    }

    // DECISION comment — render as callout
    const decisionMatch = line.match(/<!--\s*DECISION:\s*(.*?)\s*-->/);
    if (decisionMatch) {
      closeList();
      out.push(
        `<div class="decision-callout"><span class="decision-icon">⚠️</span><span>${escapeHtml(decisionMatch[1])}</span></div>`
      );
      continue;
    }

    // Horizontal rule
    if (/^---+$/.test(line.trim()) || /^\*\*\*+$/.test(line.trim())) {
      closeList();
      out.push("<hr>");
      continue;
    }

    // Headings
    const headingMatch = line.match(/^(#{1,6})\s+(.*)/);
    if (headingMatch) {
      closeList();
      const level = headingMatch[1].length;
      const text = renderInline(escapeHtml(headingMatch[2]));
      out.push(`<h${level}>${text}</h${level}>`);
      continue;
    }

    // Table rows
    if (line.trim().startsWith("|")) {
      closeList();
      const cells = line
        .split("|")
        .slice(1, -1)
        .map((c) => c.trim());
      // Separator row
      if (cells.every((c) => /^[-:]+$/.test(c))) {
        continue;
      }
      if (!inTable) {
        out.push('<table class="md-table"><thead><tr>');
        cells.forEach((c) =>
          out.push(`<th>${renderInline(escapeHtml(c))}</th>`)
        );
        out.push("</tr></thead><tbody>");
        inTable = true;
      } else {
        out.push("<tr>");
        cells.forEach((c) =>
          out.push(`<td>${renderInline(escapeHtml(c))}</td>`)
        );
        out.push("</tr>");
      }
      continue;
    } else if (inTable) {
      out.push("</tbody></table>");
      inTable = false;
    }

    // Ordered list
    const olMatch = line.match(/^(\s*)\d+\.\s+(.*)/);
    if (olMatch) {
      if (!inList || !listOrdered) {
        closeList();
        out.push("<ol>");
        inList = true;
        listOrdered = true;
      }
      out.push(`<li>${renderInline(escapeHtml(olMatch[2]))}</li>`);
      continue;
    }

    // Unordered list
    const ulMatch = line.match(/^(\s*)[-*]\s+(.*)/);
    if (ulMatch) {
      if (!inList || listOrdered) {
        closeList();
        out.push("<ul>");
        inList = true;
        listOrdered = false;
      }
      out.push(`<li>${renderInline(escapeHtml(ulMatch[2]))}</li>`);
      continue;
    }

    // Empty line
    if (line.trim() === "") {
      closeList();
      if (inTable) {
        out.push("</tbody></table>");
        inTable = false;
      }
      out.push("<p></p>");
      continue;
    }

    // Paragraph
    closeList();
    out.push(`<p>${renderInline(escapeHtml(line))}</p>`);
  }

  closeList();
  if (inTable) out.push("</tbody></table>");
  if (inCodeBlock) out.push("</code></pre>");

  return out.join("\n");
}

// ── Status chip ───────────────────────────────────────────────────────────────

const STATUS_COLORS: Record<string, string> = {
  draft: "bg-gray-700 text-gray-300",
  review: "bg-yellow-800 text-yellow-200",
  approved: "bg-green-800 text-green-200",
  implementing: "bg-blue-800 text-blue-200",
  done: "bg-gray-900 text-gray-400",
};

function StatusChip({ status }: { status: string }) {
  const cls = STATUS_COLORS[status] ?? "bg-gray-700 text-gray-300";
  return (
    <span className={`px-1.5 py-0.5 rounded text-[10px] font-medium uppercase ${cls}`}>
      {status}
    </span>
  );
}

// ── AI sidebar ────────────────────────────────────────────────────────────────

interface AiSidebarProps {
  specContent: string;
  onInsert: (text: string) => void;
  onApplyDecisions: (text: string) => void;
  onClose: () => void;
}

function AiSidebar({ specContent, onInsert, onApplyDecisions, onClose }: AiSidebarProps) {
  const [brief, setBrief] = useState("");
  const [instruction, setInstruction] = useState("");
  const [result, setResult] = useState("");
  const [loading, setLoading] = useState(false);
  const [mode, setMode] = useState<
    "generate" | "section" | "decisions" | "criteria" | "review"
  >("generate");

  const assist = async (instr: string) => {
    setLoading(true);
    setResult("");
    try {
      const text = await invoke<string>("spec_ai_assist", {
        specContent: mode === "generate" ? "" : specContent,
        instruction: instr,
      });
      setResult(text);
    } catch (e) {
      setResult(`Error: ${String(e)}`);
    } finally {
      setLoading(false);
    }
  };

  const handleGenerate = () => {
    if (!brief.trim()) return;
    assist(
      `Generate a complete software specification document in markdown with YAML frontmatter for the following feature. ` +
      `Include: frontmatter (req_id: CF-001, title, status: draft, created_at, updated_at, tags, acceptance_criteria), ` +
      `then sections: Overview, Requirements table, Decision Points (with <!-- DECISION: ... --> comments for anything ambiguous), ` +
      `and Testing Matrix. Feature description: ${brief}`
    );
  };

  const handleSection = () => {
    if (!instruction.trim()) return;
    assist(`Add a new markdown section to the spec. Return only the section markdown, no preamble. Section request: ${instruction}`);
  };

  const handleDecisions = () => {
    assist(
      `Read this spec and identify ambiguous areas, design choices, or open questions. ` +
      `Return ONLY a list of <!-- DECISION: ... --> comment lines (one per line), ` +
      `one for each decision point you find. No other text.`
    );
  };

  const handleCriteria = () => {
    assist(
      `Extract testable acceptance criteria from the spec body. ` +
      `Return ONLY a YAML list block (lines starting with "  - "), one criterion per line. No other text.`
    );
  };

  const handleReview = () => {
    assist(
      `Review this spec for completeness. Check for: Overview, Requirements table, Decision Points, ` +
      `Testing Matrix, and acceptance criteria in frontmatter. ` +
      `For each missing or thin section, explain briefly what's needed. Format as a bullet list.`
    );
  };

  return (
    <div className="w-80 flex-shrink-0 flex flex-col border-l border-border bg-surface-1">
      <div className="flex items-center gap-2 px-3 py-2 border-b border-border">
        <Sparkles size={14} className="text-accent" />
        <span className="flex-1 text-xs font-semibold text-gray-300">AI Co-drafting</span>
        <button onClick={onClose} className="p-1 rounded text-gray-600 hover:text-gray-300">
          <X size={12} />
        </button>
      </div>

      {/* Mode selector */}
      <div className="flex flex-wrap gap-1 p-2 border-b border-border">
        {(["generate", "section", "decisions", "criteria", "review"] as const).map((m) => (
          <button
            key={m}
            onClick={() => { setMode(m); setResult(""); }}
            className={`px-2 py-0.5 rounded text-[10px] capitalize transition-colors ${
              mode === m
                ? "bg-accent text-white"
                : "bg-surface-3 text-gray-400 hover:text-gray-200"
            }`}
          >
            {m === "generate" ? "Generate" :
             m === "section" ? "Add Section" :
             m === "decisions" ? "Decisions" :
             m === "criteria" ? "Criteria" : "Review"}
          </button>
        ))}
      </div>

      {/* Input area */}
      <div className="p-2 border-b border-border space-y-2">
        {mode === "generate" && (
          <>
            <textarea
              value={brief}
              onChange={(e) => setBrief(e.target.value)}
              placeholder="One-line description of the feature..."
              className="w-full bg-surface-3 border border-border rounded px-2 py-1.5 text-xs text-gray-200 placeholder-gray-600 outline-none focus:border-accent/50 resize-none"
              rows={3}
            />
            <button
              onClick={handleGenerate}
              disabled={loading || !brief.trim()}
              className="w-full px-3 py-1.5 rounded text-xs bg-accent hover:bg-accent-hover text-white disabled:opacity-50 transition-colors"
            >
              {loading ? "Generating..." : "Generate Spec"}
            </button>
          </>
        )}
        {mode === "section" && (
          <>
            <textarea
              value={instruction}
              onChange={(e) => setInstruction(e.target.value)}
              placeholder="Describe the section you want to add..."
              className="w-full bg-surface-3 border border-border rounded px-2 py-1.5 text-xs text-gray-200 placeholder-gray-600 outline-none focus:border-accent/50 resize-none"
              rows={3}
            />
            <button
              onClick={handleSection}
              disabled={loading || !instruction.trim()}
              className="w-full px-3 py-1.5 rounded text-xs bg-accent hover:bg-accent-hover text-white disabled:opacity-50 transition-colors"
            >
              {loading ? "Drafting..." : "Draft Section"}
            </button>
          </>
        )}
        {mode === "decisions" && (
          <button
            onClick={handleDecisions}
            disabled={loading}
            className="w-full px-3 py-1.5 rounded text-xs bg-accent hover:bg-accent-hover text-white disabled:opacity-50 transition-colors"
          >
            {loading ? "Analyzing..." : "Identify Decision Points"}
          </button>
        )}
        {mode === "criteria" && (
          <button
            onClick={handleCriteria}
            disabled={loading}
            className="w-full px-3 py-1.5 rounded text-xs bg-accent hover:bg-accent-hover text-white disabled:opacity-50 transition-colors"
          >
            {loading ? "Extracting..." : "Generate Criteria"}
          </button>
        )}
        {mode === "review" && (
          <button
            onClick={handleReview}
            disabled={loading}
            className="w-full px-3 py-1.5 rounded text-xs bg-accent hover:bg-accent-hover text-white disabled:opacity-50 transition-colors"
          >
            {loading ? "Reviewing..." : "Review for Completeness"}
          </button>
        )}
      </div>

      {/* Result area */}
      {result && (
        <div className="flex-1 flex flex-col min-h-0">
          <div className="flex-1 overflow-y-auto p-2">
            <pre className="text-xs text-gray-300 whitespace-pre-wrap font-mono leading-relaxed">
              {result}
            </pre>
          </div>
          <div className="flex gap-1.5 p-2 border-t border-border">
            {(mode === "generate") && (
              <button
                onClick={() => onInsert(result)}
                className="flex-1 px-2 py-1.5 rounded text-xs bg-green-700 hover:bg-green-600 text-white transition-colors"
              >
                Insert (replace editor)
              </button>
            )}
            {mode === "section" && (
              <button
                onClick={() => onInsert(result)}
                className="flex-1 px-2 py-1.5 rounded text-xs bg-green-700 hover:bg-green-600 text-white transition-colors"
              >
                Append to Spec
              </button>
            )}
            {mode === "decisions" && (
              <button
                onClick={() => onApplyDecisions(result)}
                className="flex-1 px-2 py-1.5 rounded text-xs bg-yellow-700 hover:bg-yellow-600 text-white transition-colors"
              >
                Apply Decisions
              </button>
            )}
            {mode === "criteria" && (
              <button
                onClick={() => onApplyDecisions(result)}
                className="flex-1 px-2 py-1.5 rounded text-xs bg-blue-700 hover:bg-blue-600 text-white transition-colors"
              >
                Apply Criteria
              </button>
            )}
            {mode === "review" && (
              <span className="text-xs text-gray-500 italic">Review feedback (read-only)</span>
            )}
            <button
              onClick={() => setResult("")}
              className="px-2 py-1.5 rounded text-xs bg-surface-3 text-gray-400 hover:text-gray-200 transition-colors"
            >
              Clear
            </button>
          </div>
        </div>
      )}
    </div>
  );
}

// ── Frontmatter panel ─────────────────────────────────────────────────────────

interface FrontmatterPanelProps {
  meta: SpecMeta;
  onApprove: () => void;
  onStartImplementation: () => void;
}

function FrontmatterPanel({ meta, onApprove, onStartImplementation }: FrontmatterPanelProps) {
  const canApprove = meta.status === "draft" || meta.status === "review";
  const canImplement = meta.status === "approved";

  return (
    <div className="w-56 flex-shrink-0 flex flex-col border-l border-border bg-surface-1 p-3 gap-3 overflow-y-auto">
      <div>
        <div className="text-[10px] text-gray-600 uppercase tracking-wider mb-1">Req ID</div>
        <div className="text-xs text-gray-300 font-mono">{meta.req_id ?? "—"}</div>
      </div>
      <div>
        <div className="text-[10px] text-gray-600 uppercase tracking-wider mb-1">Status</div>
        <StatusChip status={meta.status} />
      </div>
      {meta.tags.length > 0 && (
        <div>
          <div className="text-[10px] text-gray-600 uppercase tracking-wider mb-1">Tags</div>
          <div className="flex flex-wrap gap-1">
            {meta.tags.map((t) => (
              <span key={t} className="px-1.5 py-0.5 rounded bg-surface-3 text-gray-400 text-[10px]">
                {t}
              </span>
            ))}
          </div>
        </div>
      )}
      {meta.acceptance_criteria.length > 0 && (
        <div>
          <div className="text-[10px] text-gray-600 uppercase tracking-wider mb-1">
            Acceptance Criteria
          </div>
          <ul className="space-y-1">
            {meta.acceptance_criteria.map((c, i) => (
              <li key={i} className="flex items-start gap-1 text-xs text-gray-400">
                <span className="mt-0.5 text-gray-600">•</span>
                <span>{c}</span>
              </li>
            ))}
          </ul>
        </div>
      )}
      <div className="mt-auto space-y-2">
        <button
          onClick={onApprove}
          disabled={!canApprove}
          className="w-full flex items-center justify-center gap-1.5 px-3 py-1.5 rounded text-xs bg-green-800 hover:bg-green-700 text-green-100 disabled:opacity-40 transition-colors"
        >
          <CheckCircle size={12} />
          Approve Spec
        </button>
        <button
          onClick={onStartImplementation}
          disabled={!canImplement}
          className="w-full flex items-center justify-center gap-1.5 px-3 py-1.5 rounded text-xs bg-blue-800 hover:bg-blue-700 text-blue-100 disabled:opacity-40 transition-colors"
        >
          <Play size={12} />
          Start Implementation
        </button>
      </div>
    </div>
  );
}

// ── ImplementationModal ───────────────────────────────────────────────────────

interface ImplementationModalProps {
  meta: SpecMeta;
  sessionId: string;
  cwd: string;
  onConfirm: () => void;
  onCancel: () => void;
}

function ImplementationModal({ meta, sessionId, cwd, onConfirm, onCancel }: ImplementationModalProps) {
  const { createTaskTree, start } = useTasksStore();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const handleConfirm = async () => {
    setBusy(true);
    setError(null);
    try {
      const tasks = meta.acceptance_criteria.length > 0
        ? meta.acceptance_criteria.map((criterion, i) => ({
            tmp_id: `t-${i}`,
            title: criterion,
            description: `Implement and verify: ${criterion}`,
            cwd,
          }))
        : [{
            tmp_id: "t-0",
            title: meta.title,
            description: `Implement spec ${meta.req_id ?? ""}: ${meta.title}`,
            cwd,
          }];

      await createTaskTree(sessionId, tasks, []);
      await start(sessionId, meta.req_id ?? undefined, meta.title);
      onConfirm();
    } catch (e) {
      setError(String(e));
      setBusy(false);
    }
  };

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/60">
      <div className="w-96 rounded-xl border border-border bg-surface-2 shadow-2xl p-5 space-y-4">
        <h2 className="text-sm font-semibold text-gray-200">Start Implementation</h2>
        <p className="text-xs text-gray-400">
          AI will decompose this spec into{" "}
          <strong className="text-gray-200">
            {Math.max(meta.acceptance_criteria.length, 1)} task
            {meta.acceptance_criteria.length !== 1 ? "s" : ""}
          </strong>{" "}
          based on acceptance criteria and begin implementation. Continue?
        </p>
        {meta.acceptance_criteria.length > 0 && (
          <ul className="space-y-1 text-xs text-gray-500">
            {meta.acceptance_criteria.map((c, i) => (
              <li key={i} className="flex gap-1">
                <span>•</span><span>{c}</span>
              </li>
            ))}
          </ul>
        )}
        {error && <p className="text-xs text-red-400">{error}</p>}
        <div className="flex justify-end gap-2">
          <button
            onClick={onCancel}
            className="px-3 py-1.5 rounded text-xs text-gray-500 hover:text-gray-300 hover:bg-surface-3 transition-colors"
          >
            Cancel
          </button>
          <button
            onClick={handleConfirm}
            disabled={busy}
            className="px-3 py-1.5 rounded text-xs bg-accent hover:bg-accent-hover text-white disabled:opacity-50 transition-colors"
          >
            {busy ? "Starting..." : "Confirm"}
          </button>
        </div>
      </div>
    </div>
  );
}

// ── New spec modal ────────────────────────────────────────────────────────────

function NewSpecModal({ cwd, onCreated, onCancel }: {
  cwd: string;
  onCreated: () => void;
  onCancel: () => void;
}) {
  const [title, setTitle] = useState("");
  const [busy, setBusy] = useState(false);
  const { createSpec } = useSpecsStore();

  const handleCreate = async () => {
    if (!title.trim()) return;
    setBusy(true);
    try {
      await createSpec(cwd, title.trim());
      onCreated();
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/60">
      <div
        className="w-80 rounded-xl border border-border bg-surface-2 shadow-2xl p-5 space-y-4"
        onClick={(e) => e.stopPropagation()}
      >
        <h2 className="text-sm font-semibold text-gray-200">New Spec</h2>
        <input
          autoFocus
          type="text"
          value={title}
          onChange={(e) => setTitle(e.target.value)}
          onKeyDown={(e) => e.key === "Enter" && handleCreate()}
          placeholder="Feature title..."
          className="w-full bg-surface-3 border border-border rounded px-3 py-1.5 text-xs text-gray-200 placeholder-gray-600 outline-none focus:border-accent/50"
        />
        <div className="flex justify-end gap-2">
          <button
            onClick={onCancel}
            className="px-3 py-1.5 rounded text-xs text-gray-500 hover:text-gray-300 hover:bg-surface-3 transition-colors"
          >
            Cancel
          </button>
          <button
            onClick={handleCreate}
            disabled={busy || !title.trim()}
            className="px-3 py-1.5 rounded text-xs bg-accent hover:bg-accent-hover text-white disabled:opacity-50 transition-colors"
          >
            {busy ? "Creating..." : "Create"}
          </button>
        </div>
      </div>
    </div>
  );
}

// ── Import from Issue modal ────────────────────────────────────────────────────

interface ImportFromIssueModalProps {
  cwd: string;
  onImported: (path: string) => void;
  onCancel: () => void;
}

function ImportFromIssueModal({ cwd, onImported, onCancel }: ImportFromIssueModalProps) {
  const { remotes, loadRemotes, issueToSpec } = useGitRemoteStore();
  const [remoteId, setRemoteId] = useState("");
  const [repo, setRepo] = useState("");
  const [issueNumber, setIssueNumber] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    loadRemotes().then(() => {
      const s = useGitRemoteStore.getState();
      if (s.remotes.length > 0) {
        setRemoteId(s.remotes[0].id);
        setRepo(s.remotes[0].default_repo ?? "");
      }
    });
  }, [loadRemotes]);

  const handleImport = async () => {
    if (!remoteId || !repo.trim() || !issueNumber.trim()) return;
    const num = parseInt(issueNumber, 10);
    if (isNaN(num) || num <= 0) {
      setError("Issue number must be a positive integer.");
      return;
    }
    setBusy(true);
    setError(null);
    try {
      const path = await issueToSpec(remoteId, repo.trim(), num, cwd);
      onImported(path);
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  if (remotes.length === 0) {
    return (
      <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/60">
        <div className="w-80 rounded-xl border border-border bg-surface-2 shadow-2xl p-5 space-y-4">
          <h2 className="text-sm font-semibold text-gray-200">Import from Issue</h2>
          <p className="text-xs text-gray-500">No remotes configured. Add a GitHub or GitLab remote in Settings first.</p>
          <div className="flex justify-end">
            <button onClick={onCancel} className="px-3 py-1.5 rounded text-xs text-gray-500 hover:text-gray-300 hover:bg-surface-3 transition-colors">
              Close
            </button>
          </div>
        </div>
      </div>
    );
  }

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/60">
      <div className="w-96 rounded-xl border border-border bg-surface-2 shadow-2xl p-5 space-y-4" onClick={(e) => e.stopPropagation()}>
        <h2 className="text-sm font-semibold text-gray-200">Import from Issue</h2>

        <div className="space-y-3">
          <div>
            <label className="block text-[10px] text-gray-600 uppercase tracking-wider mb-1">Remote</label>
            <select
              value={remoteId}
              onChange={(e) => {
                setRemoteId(e.target.value);
                const r = remotes.find((x) => x.id === e.target.value);
                if (r?.default_repo) setRepo(r.default_repo);
              }}
              className="w-full bg-surface-3 border border-border rounded px-2 py-1.5 text-xs text-gray-200 outline-none focus:border-accent/50"
            >
              {remotes.map((r) => (
                <option key={r.id} value={r.id}>{r.name} ({r.provider})</option>
              ))}
            </select>
          </div>
          <div>
            <label className="block text-[10px] text-gray-600 uppercase tracking-wider mb-1">Repository</label>
            <input
              type="text"
              value={repo}
              onChange={(e) => setRepo(e.target.value)}
              placeholder="owner/repo"
              className="w-full bg-surface-3 border border-border rounded px-2 py-1.5 text-xs text-gray-200 placeholder-gray-600 outline-none focus:border-accent/50"
            />
          </div>
          <div>
            <label className="block text-[10px] text-gray-600 uppercase tracking-wider mb-1">Issue Number</label>
            <input
              autoFocus
              type="number"
              min={1}
              value={issueNumber}
              onChange={(e) => setIssueNumber(e.target.value)}
              onKeyDown={(e) => e.key === "Enter" && handleImport()}
              placeholder="123"
              className="w-full bg-surface-3 border border-border rounded px-2 py-1.5 text-xs text-gray-200 placeholder-gray-600 outline-none focus:border-accent/50"
            />
          </div>
          {error && <p className="text-xs text-red-400">{error}</p>}
        </div>

        <div className="flex justify-end gap-2">
          <button
            onClick={onCancel}
            className="px-3 py-1.5 rounded text-xs text-gray-500 hover:text-gray-300 hover:bg-surface-3 transition-colors"
          >
            Cancel
          </button>
          <button
            onClick={handleImport}
            disabled={busy || !repo.trim() || !issueNumber.trim()}
            className="px-3 py-1.5 rounded text-xs bg-accent hover:bg-accent-hover text-white disabled:opacity-50 transition-colors"
          >
            {busy ? "Importing..." : "Import"}
          </button>
        </div>
      </div>
    </div>
  );
}

// ── Main SpecsPage ────────────────────────────────────────────────────────────

interface SpecsPageProps {
  onBack: () => void;
}

export function SpecsPage({ onBack }: SpecsPageProps) {
  const { activeSession } = useChatStore();
  const cwd = activeSession?.cwd ?? "";

  const {
    specs,
    activeSpec,
    loading,
    loadSpecs,
    openSpec,
    saveSpec,
    deleteSpec,
    approveSpec,
    updateActiveContent,
  } = useSpecsStore();

  const [tab, setTab] = useState<"edit" | "preview" | "evidence">("edit");
  const [filter, setFilter] = useState("");
  const [aiOpen, setAiOpen] = useState(false);
  const [newSpecOpen, setNewSpecOpen] = useState(false);
  const [importIssueOpen, setImportIssueOpen] = useState(false);
  const [implModal, setImplModal] = useState(false);
  const [evidenceViewerPath, setEvidenceViewerPath] = useState<string | null>(null);
  // Map of req_id -> evidence pack count for sidebar badges
  const [evidenceCountMap, setEvidenceCountMap] = useState<Record<string, number>>({});

  // Debounced auto-save
  const saveTimer = useRef<ReturnType<typeof setTimeout> | null>(null);

  useEffect(() => {
    if (cwd) loadSpecs(cwd);
  }, [cwd]);

  // Load evidence pack counts for sidebar badges
  useEffect(() => {
    if (!cwd) return;
    invoke<Array<{ spec_req_id: string }>>("list_evidence_packs", { cwd })
      .then((packs) => {
        const counts: Record<string, number> = {};
        for (const p of packs) {
          counts[p.spec_req_id] = (counts[p.spec_req_id] ?? 0) + 1;
        }
        setEvidenceCountMap(counts);
      })
      .catch(() => {});
  }, [cwd]);

  const handleEditorChange = useCallback(
    (content: string) => {
      updateActiveContent(content);
      if (saveTimer.current) clearTimeout(saveTimer.current);
      saveTimer.current = setTimeout(() => {
        if (activeSpec) {
          saveSpec(activeSpec.meta.file_path, content).catch(() => {});
        }
      }, 1000);
    },
    [activeSpec, saveSpec, updateActiveContent]
  );

  const handleApprove = async () => {
    if (!activeSpec) return;
    await approveSpec(activeSpec.meta.file_path);
  };

  const handleApplyDecisions = (text: string) => {
    if (!activeSpec) return;
    // Append the decision callouts before the last line
    const newContent = activeSpec.content + "\n\n" + text;
    handleEditorChange(newContent);
  };

  const filteredSpecs = specs.filter(
    (s) =>
      s.title.toLowerCase().includes(filter.toLowerCase()) ||
      (s.req_id ?? "").toLowerCase().includes(filter.toLowerCase())
  );

  const noCwd = !cwd;

  return (
    <div className="flex h-full bg-surface-0">
      {/* Left sidebar */}
      <aside className="w-60 flex-shrink-0 flex flex-col border-r border-border bg-surface-1">
        {/* Header */}
        <div className="flex items-center gap-1 px-3 py-2 border-b border-border">
          <button
            onClick={onBack}
            className="p-1 rounded text-gray-600 hover:text-gray-300 hover:bg-surface-3 transition-colors"
            title="Back to Chat"
          >
            <ChevronLeft size={14} />
          </button>
          <span className="flex-1 text-xs font-semibold text-gray-400 uppercase tracking-wider">
            Specs
          </span>
          <button
            onClick={() => setImportIssueOpen(true)}
            disabled={noCwd}
            className="p-1 rounded hover:bg-surface-3 text-gray-500 hover:text-gray-300 disabled:opacity-40 transition-colors"
            title="Import from Issue"
          >
            <Download size={14} />
          </button>
          <button
            onClick={() => setNewSpecOpen(true)}
            disabled={noCwd}
            className="p-1 rounded hover:bg-surface-3 text-gray-500 hover:text-gray-300 disabled:opacity-40 transition-colors"
            title="New spec"
          >
            <Plus size={14} />
          </button>
        </div>

        {/* Search */}
        <div className="px-2 py-1.5 border-b border-border">
          <input
            type="text"
            value={filter}
            onChange={(e) => setFilter(e.target.value)}
            placeholder="Filter specs..."
            className="w-full bg-surface-3 border border-border rounded px-2 py-1 text-xs text-gray-300 placeholder-gray-600 outline-none focus:border-accent/50"
          />
        </div>

        {/* Spec list */}
        <ul className="flex-1 overflow-y-auto py-1">
          {noCwd && (
            <li className="px-3 py-2 text-xs text-gray-700">Open a project first</li>
          )}
          {!noCwd && loading && (
            <li className="px-3 py-2 text-xs text-gray-700">Loading...</li>
          )}
          {!noCwd && !loading && filteredSpecs.length === 0 && (
            <li className="px-3 py-2 text-xs text-gray-700">No specs yet</li>
          )}
          {filteredSpecs.map((s) => (
            <li key={s.file_path}>
              <button
                className={`group w-full flex flex-col gap-0.5 px-3 py-2 text-left transition-colors ${
                  activeSpec?.meta.file_path === s.file_path
                    ? "bg-surface-3 text-gray-200"
                    : "text-gray-500 hover:bg-surface-2 hover:text-gray-300"
                }`}
                onClick={() => openSpec(s.file_path)}
              >
                <div className="flex items-center gap-1.5 w-full min-w-0">
                  {s.req_id && (
                    <span className="flex-shrink-0 text-[10px] font-mono text-accent">
                      {s.req_id}
                    </span>
                  )}
                  <span className="flex-1 truncate text-xs">{s.title}</span>
                  <span
                    className="opacity-0 group-hover:opacity-100 transition-opacity p-0.5 rounded hover:bg-surface-4 text-gray-600 hover:text-red-400"
                    role="button"
                    title="Delete"
                    onClick={(e) => {
                      e.stopPropagation();
                      deleteSpec(s.file_path);
                    }}
                  >
                    <Trash2 size={10} />
                  </span>
                </div>
                <div className="flex items-center gap-1.5">
                  <StatusChip status={s.status} />
                  {s.req_id && evidenceCountMap[s.req_id] ? (
                    <span className="flex items-center gap-0.5 px-1 py-0.5 rounded bg-surface-3 text-[9px] text-gray-500">
                      <Archive size={8} />
                      {evidenceCountMap[s.req_id]}
                    </span>
                  ) : null}
                </div>
              </button>
            </li>
          ))}
        </ul>
      </aside>

      {/* Main editor area */}
      <div className="flex flex-1 flex-col min-w-0">
        {!activeSpec ? (
          <div className="flex-1 flex items-center justify-center text-sm text-gray-700">
            {noCwd
              ? "Open a project session to manage specs."
              : "Select a spec or create a new one."}
          </div>
        ) : (
          <>
            {/* Tab bar + AI button */}
            <div className="flex items-center gap-0 border-b border-border bg-surface-1 px-3 shrink-0">
              <div className="flex gap-0 mr-auto">
                {(["edit", "preview", "evidence"] as const).map((t) => (
                  <button
                    key={t}
                    onClick={() => setTab(t)}
                    className={`flex items-center gap-1 px-4 py-2 text-xs capitalize border-b-2 transition-colors ${
                      tab === t
                        ? "border-accent text-gray-200"
                        : "border-transparent text-gray-500 hover:text-gray-300"
                    }`}
                  >
                    {t === "evidence" && <Archive size={11} />}
                    {t}
                  </button>
                ))}
              </div>
              <span className="text-[10px] text-gray-600 truncate max-w-xs px-2">
                {activeSpec.meta.rel_path}
              </span>
              <button
                onClick={() => setAiOpen((o) => !o)}
                className={`flex items-center gap-1 px-2 py-1.5 rounded text-xs transition-colors ml-2 ${
                  aiOpen
                    ? "text-accent bg-surface-3"
                    : "text-gray-500 hover:text-gray-300 hover:bg-surface-3"
                }`}
                title="AI Co-drafting"
              >
                <Sparkles size={12} />
                <span>AI</span>
              </button>
            </div>

            {/* Editor + right panels */}
            <div className="flex flex-1 min-h-0">
              {/* Edit / Preview / Evidence */}
              <div className="flex-1 flex flex-col min-w-0">
                {tab === "edit" ? (
                  <textarea
                    value={activeSpec.content}
                    onChange={(e) => handleEditorChange(e.target.value)}
                    className="flex-1 w-full bg-surface-0 text-gray-200 text-xs font-mono p-4 resize-none outline-none leading-relaxed"
                    spellCheck={false}
                  />
                ) : tab === "preview" ? (
                  <div className="flex-1 overflow-y-auto p-6">
                    <div
                      className="spec-preview max-w-3xl"
                      dangerouslySetInnerHTML={{
                        __html: renderMarkdown(activeSpec.body || activeSpec.content),
                      }}
                    />
                  </div>
                ) : (
                  <EvidencePackList
                    specReqId={activeSpec.meta.req_id ?? ""}
                    cwd={cwd}
                  />
                )}
              </div>

              {/* Frontmatter panel */}
              <FrontmatterPanel
                meta={activeSpec.meta}
                onApprove={handleApprove}
                onStartImplementation={() => setImplModal(true)}
              />

              {/* AI sidebar */}
              {aiOpen && (
                <AiSidebar
                  specContent={activeSpec.content}
                  onInsert={(text) => handleEditorChange(
                    // "generate" replaces; "section" appends
                    text.startsWith("---") ? text : (activeSpec.content + "\n\n" + text)
                  )}
                  onApplyDecisions={handleApplyDecisions}
                  onClose={() => setAiOpen(false)}
                />
              )}
            </div>
          </>
        )}
      </div>

      {/* Modals */}
      {newSpecOpen && (
        <NewSpecModal
          cwd={cwd}
          onCreated={() => {
            setNewSpecOpen(false);
            loadSpecs(cwd);
          }}
          onCancel={() => setNewSpecOpen(false)}
        />
      )}

      {importIssueOpen && (
        <ImportFromIssueModal
          cwd={cwd}
          onImported={(path) => {
            setImportIssueOpen(false);
            loadSpecs(cwd);
            // Open the newly created spec
            openSpec(path);
          }}
          onCancel={() => setImportIssueOpen(false)}
        />
      )}

      {implModal && activeSpec && activeSession && (
        <ImplementationModal
          meta={activeSpec.meta}
          sessionId={activeSession.id}
          cwd={cwd}
          onConfirm={() => {
            setImplModal(false);
            onBack(); // Navigate back to chat/tasks view
          }}
          onCancel={() => setImplModal(false)}
        />
      )}

      {evidenceViewerPath && (
        <EvidenceViewer
          packPath={evidenceViewerPath}
          onClose={() => setEvidenceViewerPath(null)}
        />
      )}
    </div>
  );
}
