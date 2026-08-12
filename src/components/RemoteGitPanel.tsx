// SPDX-License-Identifier: Apache-2.0
import { type KeyboardEvent, useEffect, useRef, useState } from "react";
import {
  X,
  GitPullRequest,
  CircleDot,
  RefreshCw,
  Plus,
  ExternalLink,
  ChevronLeft,
  Tag,
} from "lucide-react";
import { invoke } from "../lib/tauri";
import { useGitRemoteStore } from "../stores/gitRemote";
import type { RemoteIssue, GitRemoteConfig } from "../lib/tauri";

// ── Open URL in system browser ────────────────────────────────────────────────

async function openUrl(url: string) {
  try {
    await invoke("plugin:shell|open", { path: url });
  } catch {
    // fallback
    window.open(url, "_blank");
  }
}

// ── Markdown renderer (minimal, inline only) ──────────────────────────────────

function renderBody(text: string): string {
  if (!text) return "<em class='text-gray-600'>无描述</em>";
  // Basic inline transforms
  let out = text
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/\*\*(.+?)\*\*/g, "<strong>$1</strong>")
    .replace(/`([^`]+)`/g, "<code>$1</code>");
  // Newlines → <br>
  out = out.replace(/\n/g, "<br>");
  return out;
}

// ── Issue detail view ─────────────────────────────────────────────────────────

interface IssueDetailProps {
  issue: RemoteIssue;
  onBack: () => void;
  largeTargets: boolean;
}

function IssueDetail({ issue, onBack, largeTargets }: IssueDetailProps) {
  return (
    <div className="flex flex-col h-full">
      <div className="flex items-center gap-2 px-3 py-2 border-b border-border shrink-0">
        <button
          onClick={onBack}
          aria-label="返回问题列表"
          className={`inline-flex shrink-0 items-center justify-center rounded text-gray-600 transition-colors hover:bg-surface-3 hover:text-gray-300 ${largeTargets ? "h-11 w-11" : "h-9 w-9"}`}
        >
          <ChevronLeft size={14} />
        </button>
        <span className="flex-1 text-label text-gray-400 truncate">
          #{issue.number} {issue.title}
        </span>
        <button
          onClick={() => openUrl(issue.url)}
          aria-label="在浏览器中打开问题"
          className={`inline-flex shrink-0 items-center justify-center rounded text-gray-600 transition-colors hover:bg-surface-3 hover:text-gray-300 ${largeTargets ? "h-11 w-11" : "h-9 w-9"}`}
          title="在浏览器中打开"
        >
          <ExternalLink size={14} />
        </button>
      </div>

      <div className="flex-1 overflow-y-auto p-4 space-y-3">
        <h2 className="text-body font-semibold text-gray-200">{issue.title}</h2>

        <div className="flex flex-wrap gap-1.5 text-caption">
          <span
            className={`px-1.5 py-0.5 rounded ${
              issue.state === "open"
                ? "bg-status-progress-soft text-status-progress"
                : "bg-gray-700 text-gray-400"
            }`}
          >
            {issue.state}
          </span>
          {issue.labels.map((l) => (
            <span
              key={l}
              className="px-1.5 py-0.5 rounded bg-status-info-soft text-status-info flex items-center gap-0.5"
            >
              <Tag size={14} />
              {l}
            </span>
          ))}
        </div>

        <div className="text-caption text-gray-600">
          作者 <span className="text-gray-400">{issue.author}</span> ·{" "}
          {new Date(issue.created_at).toLocaleDateString()}
        </div>

        <div
          className="text-label text-gray-300 leading-relaxed border border-border rounded p-3 bg-surface-0 prose-sm"
          dangerouslySetInnerHTML={{ __html: renderBody(issue.body) }}
        />
      </div>

    </div>
  );
}

// ── New Issue form ────────────────────────────────────────────────────────────

interface NewIssueFormProps {
  remoteId: string;
  repo: string;
  onCreated: () => void;
  onCancel: () => void;
  largeTargets: boolean;
}

function NewIssueForm({ remoteId, repo, onCreated, onCancel, largeTargets }: NewIssueFormProps) {
  const { createIssue } = useGitRemoteStore();
  const [title, setTitle] = useState("");
  const [body, setBody] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const handleSubmit = async () => {
    if (!title.trim()) return;
    setBusy(true);
    setError(null);
    try {
      await createIssue(remoteId, repo, title.trim(), body.trim(), []);
      onCreated();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="flex flex-col h-full">
      <div className="flex items-center gap-2 px-3 py-2 border-b border-border shrink-0">
        <button
          onClick={onCancel}
          aria-label="返回问题列表"
          className={`inline-flex shrink-0 items-center justify-center rounded text-gray-600 hover:bg-surface-3 hover:text-gray-300 ${largeTargets ? "h-11 w-11" : "h-9 w-9"}`}
        >
          <ChevronLeft size={14} />
        </button>
        <span className="text-label font-semibold text-gray-300">新建问题</span>
      </div>

      <div className="flex-1 overflow-y-auto p-3 space-y-3">
        <div>
          <label className="block text-caption text-gray-600 mb-1">标题</label>
          <input
            autoFocus
            type="text"
            value={title}
            onChange={(e) => setTitle(e.target.value)}
            placeholder="问题标题…"
            className="w-full bg-surface-3 border border-border rounded px-2 py-1.5 text-label text-gray-200 placeholder-gray-600 outline-none focus:border-accent/50"
          />
        </div>
        <div>
          <label className="block text-caption text-gray-600 mb-1">描述</label>
          <textarea
            value={body}
            onChange={(e) => setBody(e.target.value)}
            placeholder="描述问题…"
            rows={8}
            className="w-full bg-surface-3 border border-border rounded px-2 py-1.5 text-label text-gray-200 placeholder-gray-600 outline-none focus:border-accent/50 resize-none"
          />
        </div>
        {error && <p className="text-label text-status-danger">{error}</p>}
      </div>

      <div className="flex gap-2 p-3 border-t border-border shrink-0">
        <button
          onClick={onCancel}
          className={`${largeTargets ? "h-11" : "h-9"} px-3 rounded text-label text-gray-500 hover:text-gray-300 hover:bg-surface-3 transition-colors`}
        >
          取消
        </button>
        <button
          onClick={handleSubmit}
          disabled={busy || !title.trim()}
          className={`${largeTargets ? "h-11" : "h-9"} flex-1 px-3 rounded text-label bg-accent hover:bg-accent-hover text-white disabled:opacity-50 transition-colors`}
        >
          {busy ? "创建中…" : "创建问题"}
        </button>
      </div>
    </div>
  );
}

// ── New PR form ───────────────────────────────────────────────────────────────

interface NewPRFormProps {
  remoteId: string;
  repo: string;
  currentBranch: string;
  onCreated: () => void;
  onCancel: () => void;
  largeTargets: boolean;
}

function NewPRForm({ remoteId, repo, currentBranch, onCreated, onCancel, largeTargets }: NewPRFormProps) {
  const { createPR } = useGitRemoteStore();
  const [title, setTitle] = useState("");
  const [body, setBody] = useState("");
  const [head, setHead] = useState(currentBranch);
  const [base, setBase] = useState("main");
  const [draft, setDraft] = useState(true);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const handleSubmit = async () => {
    if (!title.trim() || !head.trim() || !base.trim()) return;
    setBusy(true);
    setError(null);
    try {
      const pr = await createPR(remoteId, repo, title.trim(), body.trim(), head.trim(), base.trim(), draft);
      openUrl(pr.url);
      onCreated();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="flex flex-col h-full">
      <div className="flex items-center gap-2 px-3 py-2 border-b border-border shrink-0">
        <button
          onClick={onCancel}
          aria-label="返回拉取请求列表"
          className={`inline-flex shrink-0 items-center justify-center rounded text-gray-600 hover:bg-surface-3 hover:text-gray-300 ${largeTargets ? "h-11 w-11" : "h-9 w-9"}`}
        >
          <ChevronLeft size={14} />
        </button>
        <span className="text-label font-semibold text-gray-300">创建拉取请求</span>
      </div>

      <div className="flex-1 overflow-y-auto p-3 space-y-3">
        <div>
          <label className="block text-caption text-gray-600 mb-1">标题</label>
          <input
            autoFocus
            type="text"
            value={title}
            onChange={(e) => setTitle(e.target.value)}
            placeholder="拉取请求标题…"
            className="w-full bg-surface-3 border border-border rounded px-2 py-1.5 text-label text-gray-200 placeholder-gray-600 outline-none focus:border-accent/50"
          />
        </div>
        <div className="flex gap-2">
          <div className="flex-1">
            <label className="block text-caption text-gray-600 mb-1">源分支</label>
            <input
              type="text"
              value={head}
              onChange={(e) => setHead(e.target.value)}
              className="w-full bg-surface-3 border border-border rounded px-2 py-1.5 text-label text-gray-200 outline-none focus:border-accent/50"
            />
          </div>
          <div className="flex-1">
            <label className="block text-caption text-gray-600 mb-1">目标分支</label>
            <input
              type="text"
              value={base}
              onChange={(e) => setBase(e.target.value)}
              className="w-full bg-surface-3 border border-border rounded px-2 py-1.5 text-label text-gray-200 outline-none focus:border-accent/50"
            />
          </div>
        </div>
        <div>
          <label className="block text-caption text-gray-600 mb-1">描述</label>
          <textarea
            value={body}
            onChange={(e) => setBody(e.target.value)}
            placeholder="描述变更…"
            rows={6}
            className="w-full bg-surface-3 border border-border rounded px-2 py-1.5 text-label text-gray-200 placeholder-gray-600 outline-none focus:border-accent/50 resize-none"
          />
        </div>
        <label className="flex items-center gap-2 text-label text-gray-400 cursor-pointer">
          <input
            type="checkbox"
            checked={draft}
            onChange={(e) => setDraft(e.target.checked)}
            className="accent-accent"
          />
          草稿拉取请求
        </label>
        {error && <p className="text-label text-status-danger">{error}</p>}
      </div>

      <div className="flex gap-2 p-3 border-t border-border shrink-0">
        <button
          onClick={onCancel}
          className={`${largeTargets ? "h-11" : "h-9"} px-3 rounded text-label text-gray-500 hover:text-gray-300 hover:bg-surface-3 transition-colors`}
        >
          取消
        </button>
        <button
          onClick={handleSubmit}
          disabled={busy || !title.trim()}
          className={`${largeTargets ? "h-11" : "h-9"} flex-1 px-3 rounded text-label bg-accent hover:bg-accent-hover text-white disabled:opacity-50 transition-colors`}
        >
          {busy ? "创建中…" : "创建拉取请求"}
        </button>
      </div>
    </div>
  );
}

// ── Issues tab ────────────────────────────────────────────────────────────────

interface IssuesTabProps {
  remotes: GitRemoteConfig[];
  largeTargets: boolean;
}

function IssuesTab({ remotes, largeTargets }: IssuesTabProps) {
  const { issues, loading, error, loadIssues } = useGitRemoteStore();
  const [remoteId, setRemoteId] = useState(remotes[0]?.id ?? "");
  const [repo, setRepo] = useState(remotes[0]?.default_repo ?? "");
  const [stateFilter, setStateFilter] = useState("open");
  const [selectedIssue, setSelectedIssue] = useState<RemoteIssue | null>(null);
  const [newIssueOpen, setNewIssueOpen] = useState(false);

  const selectedRemote = remotes.find((r) => r.id === remoteId);
  if (!selectedRemote) {
    return <div className="flex-1 flex-col" />
  }

  // Sync repo when remote changes
  const handleRemoteChange = (id: string) => {
    setRemoteId(id);
    const r = remotes.find((x) => x.id === id);
    if (r?.default_repo) setRepo(r.default_repo);
  };

  const handleLoad = () => {
    if (remoteId && repo) {
      loadIssues(remoteId, repo, stateFilter);
      setSelectedIssue(null);
    }
  };

  useEffect(() => {
    if (remoteId && repo) loadIssues(remoteId, repo, stateFilter);
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  if (selectedIssue) {
    return (
      <IssueDetail
        issue={selectedIssue}
        largeTargets={largeTargets}
        onBack={() => setSelectedIssue(null)}
      />
    );
  }

  if (newIssueOpen) {
    return (
      <NewIssueForm
        remoteId={remoteId}
        repo={repo}
        largeTargets={largeTargets}
        onCreated={() => {
          setNewIssueOpen(false);
          handleLoad();
        }}
        onCancel={() => setNewIssueOpen(false)}
      />
    );
  }

  return (
    <div className="flex flex-col h-full">
      {/* Controls */}
      <div className="flex flex-col gap-2 p-3 border-b border-border shrink-0">
        <div className="flex gap-2">
          <select
            value={remoteId}
            onChange={(e) => handleRemoteChange(e.target.value)}
            className={`${largeTargets ? "h-11" : "h-9"} flex-1 bg-surface-3 border border-border rounded px-2 text-label text-gray-300 outline-none focus:border-accent/50`}
          >
            {remotes.map((r) => (
              <option key={r.id} value={r.id}>
                {r.name}
              </option>
            ))}
          </select>
          <select
            value={stateFilter}
            onChange={(e) => setStateFilter(e.target.value)}
            className={`${largeTargets ? "h-11" : "h-9"} bg-surface-3 border border-border rounded px-2 text-label text-gray-300 outline-none focus:border-accent/50`}
          >
            <option value="open">开放</option>
            <option value="closed">已关闭</option>
            <option value="all">全部</option>
          </select>
        </div>
        <div className="flex gap-2">
          <input
            type="text"
            value={repo}
            onChange={(e) => setRepo(e.target.value)}
            placeholder="owner/repo"
            className={`${largeTargets ? "h-11" : "h-9"} flex-1 bg-surface-3 border border-border rounded px-2 text-label text-gray-200 placeholder-gray-600 outline-none focus:border-accent/50`}
          />
          <button
            onClick={handleLoad}
            disabled={!repo || loading}
            aria-label="刷新问题"
            className={`inline-flex shrink-0 items-center justify-center rounded bg-surface-3 text-gray-400 transition-colors hover:text-gray-200 disabled:opacity-50 ${largeTargets ? "h-11 w-11" : "h-9 w-9"}`}
            title="刷新"
          >
            <RefreshCw size={14} className={loading ? "animate-spin motion-reduce:animate-none" : ""} />
          </button>
          <button
            onClick={() => setNewIssueOpen(true)}
            disabled={!repo}
            aria-label="新建问题"
            className={`inline-flex shrink-0 items-center justify-center rounded bg-surface-3 text-gray-400 transition-colors hover:text-gray-200 disabled:opacity-50 ${largeTargets ? "h-11 w-11" : "h-9 w-9"}`}
            title="新建问题"
          >
            <Plus size={14} />
          </button>
        </div>
      </div>

      {/* Issue list */}
      <div className="flex-1 overflow-y-auto">
        {error && (
          <div className="px-3 py-2 text-label text-status-danger border-b border-border">{error}</div>
        )}
        {!loading && issues.length === 0 && (
          <div className="px-3 py-3 text-label text-gray-600">
            {repo ? "未找到问题。" : "输入仓库以加载问题。"}
          </div>
        )}
        {issues.map((issue) => (
          <button
            key={issue.id}
            onClick={() => setSelectedIssue(issue)}
            className="w-full text-left px-3 py-2.5 border-b border-border hover:bg-surface-2 transition-colors group"
          >
            <div className="flex items-start gap-2">
              <CircleDot
                size={14}
                className={`mt-0.5 shrink-0 ${issue.state === "open" ? "text-status-progress" : "text-gray-600"}`}
              />
              <div className="flex-1 min-w-0">
                <div className="flex items-center gap-1.5">
                  <span className="text-caption text-gray-600 font-mono shrink-0">#{issue.number}</span>
                  <span className="text-label text-gray-300 truncate">{issue.title}</span>
                </div>
                {issue.labels.length > 0 && (
                  <div className="flex flex-wrap gap-1 mt-0.5">
                    {issue.labels.slice(0, 3).map((l) => (
                      <span key={l} className="text-caption px-1 py-0 rounded bg-surface-3 text-gray-500">
                        {l}
                      </span>
                    ))}
                  </div>
                )}
                <div className="mt-0.5 text-caption text-gray-600">
                  {issue.author} · {new Date(issue.created_at).toLocaleDateString()}
                </div>
              </div>
            </div>
          </button>
        ))}
      </div>
    </div>
  );
}

// ── PRs tab ───────────────────────────────────────────────────────────────────

interface PRsTabProps {
  remotes: GitRemoteConfig[];
  currentBranch: string;
  largeTargets: boolean;
}

function PRsTab({ remotes, currentBranch, largeTargets }: PRsTabProps) {
  const { prs, loading, error, loadPRs } = useGitRemoteStore();
  const [remoteId, setRemoteId] = useState(remotes[0]?.id ?? "");
  const [repo, setRepo] = useState(remotes[0]?.default_repo ?? "");
  const [stateFilter, setStateFilter] = useState("open");
  const [newPROpen, setNewPROpen] = useState(false);

  const handleLoad = () => {
    if (remoteId && repo) loadPRs(remoteId, repo, stateFilter);
  };

  useEffect(() => {
    if (remoteId && repo) loadPRs(remoteId, repo, stateFilter);
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const handleRemoteChange = (id: string) => {
    setRemoteId(id);
    const r = remotes.find((x) => x.id === id);
    if (r?.default_repo) setRepo(r.default_repo);
  };

  if (newPROpen) {
    return (
      <NewPRForm
        remoteId={remoteId}
        repo={repo}
        currentBranch={currentBranch}
        largeTargets={largeTargets}
        onCreated={() => {
          setNewPROpen(false);
          handleLoad();
        }}
        onCancel={() => setNewPROpen(false)}
      />
    );
  }

  const prStateColor = (state: string) => {
    switch (state) {
      case "open": return "text-status-progress";
      case "merged": return "text-status-success";
      default: return "text-gray-600";
    }
  };

  return (
    <div className="flex flex-col h-full">
      {/* Controls */}
      <div className="flex flex-col gap-2 p-3 border-b border-border shrink-0">
        <div className="flex gap-2">
          <select
            value={remoteId}
            onChange={(e) => handleRemoteChange(e.target.value)}
            className={`${largeTargets ? "h-11" : "h-9"} flex-1 bg-surface-3 border border-border rounded px-2 text-label text-gray-300 outline-none focus:border-accent/50`}
          >
            {remotes.map((r) => (
              <option key={r.id} value={r.id}>{r.name}</option>
            ))}
          </select>
          <select
            value={stateFilter}
            onChange={(e) => setStateFilter(e.target.value)}
            className={`${largeTargets ? "h-11" : "h-9"} bg-surface-3 border border-border rounded px-2 text-label text-gray-300 outline-none focus:border-accent/50`}
          >
            <option value="open">开放</option>
            <option value="closed">已关闭</option>
            <option value="all">全部</option>
          </select>
        </div>
        <div className="flex gap-2">
          <input
            type="text"
            value={repo}
            onChange={(e) => setRepo(e.target.value)}
            placeholder="owner/repo"
            className={`${largeTargets ? "h-11" : "h-9"} flex-1 bg-surface-3 border border-border rounded px-2 text-label text-gray-200 placeholder-gray-600 outline-none focus:border-accent/50`}
          />
          <button
            onClick={handleLoad}
            disabled={!repo || loading}
            aria-label="刷新拉取请求"
            className={`inline-flex shrink-0 items-center justify-center rounded bg-surface-3 text-gray-400 transition-colors hover:text-gray-200 disabled:opacity-50 ${largeTargets ? "h-11 w-11" : "h-9 w-9"}`}
            title="刷新"
          >
            <RefreshCw size={14} className={loading ? "animate-spin motion-reduce:animate-none" : ""} />
          </button>
          <button
            onClick={() => setNewPROpen(true)}
            disabled={!repo}
            aria-label="创建拉取请求"
            className={`inline-flex shrink-0 items-center justify-center rounded bg-surface-3 text-gray-400 transition-colors hover:text-gray-200 disabled:opacity-50 ${largeTargets ? "h-11 w-11" : "h-9 w-9"}`}
            title="创建拉取请求"
          >
            <Plus size={14} />
          </button>
        </div>
      </div>

      {/* PR list */}
      <div className="flex-1 overflow-y-auto">
        {error && (
          <div className="px-3 py-2 text-label text-status-danger border-b border-border">{error}</div>
        )}
        {!loading && prs.length === 0 && (
          <div className="px-3 py-3 text-label text-gray-600">
            {repo ? "未找到拉取请求。" : "输入仓库以加载拉取请求。"}
          </div>
        )}
        {prs.map((pr) => (
          <button
            key={pr.id}
            onClick={() => openUrl(pr.url)}
            className="w-full text-left px-3 py-2.5 border-b border-border hover:bg-surface-2 transition-colors group"
          >
            <div className="flex items-start gap-2">
              <GitPullRequest size={14} className={`mt-0.5 shrink-0 ${prStateColor(pr.state)}`} />
              <div className="flex-1 min-w-0">
                <div className="flex items-center gap-1.5">
                  <span className="text-caption text-gray-600 font-mono shrink-0">#{pr.number}</span>
                  <span className="text-label text-gray-300 truncate">{pr.title}</span>
                  {pr.draft && (
                    <span className="text-caption px-1 rounded bg-surface-3 text-gray-500 shrink-0">草稿</span>
                  )}
                </div>
                <div className="mt-0.5 font-mono text-caption text-gray-600">
                  {pr.head_branch} → {pr.base_branch}
                </div>
                <div className="flex items-center gap-1 mt-0.5">
                  <span className="text-caption text-gray-600">
                    {new Date(pr.created_at).toLocaleDateString()}
                  </span>
                  <ExternalLink size={14} className="text-gray-600 opacity-0 transition-opacity group-hover:opacity-100" />
                </div>
              </div>
            </div>
          </button>
        ))}
      </div>
    </div>
  );
}

// ── Main RemoteGitPanel ───────────────────────────────────────────────────────

interface RemoteGitPanelProps {
  currentBranch: string;
  onClose: () => void;
  embedded?: boolean;
}

export function RemoteGitPanel({ currentBranch, onClose, embedded = false }: RemoteGitPanelProps) {
  const { remotes, loadRemotes } = useGitRemoteStore();
  const [tab, setTab] = useState<"issues" | "prs">("issues");
  const [isNarrowEmbedded, setIsNarrowEmbedded] = useState(embedded);
  const panelRef = useRef<HTMLDivElement>(null);
  const issuesTabRef = useRef<HTMLButtonElement>(null);
  const prsTabRef = useRef<HTMLButtonElement>(null);

  useEffect(() => {
    if (!embedded) {
      setIsNarrowEmbedded(false);
      return;
    }
    const panel = panelRef.current;
    if (!panel) return;
    const update = () => setIsNarrowEmbedded(panel.getBoundingClientRect().width < 640);
    update();
    if (typeof ResizeObserver === "undefined") return;
    const observer = new ResizeObserver(update);
    observer.observe(panel);
    return () => observer.disconnect();
  }, [embedded]);

  useEffect(() => {
    loadRemotes();
  }, [loadRemotes]);

  const largeTargets = embedded && isNarrowEmbedded;
  const activeTabId = `remote-git-tab-${tab}`;
  const activePanelId = `remote-git-tabpanel-${tab}`;

  const handleTabKeyDown = (
    event: KeyboardEvent<HTMLButtonElement>,
    currentTab: "issues" | "prs",
  ) => {
    if (event.key !== "ArrowLeft" && event.key !== "ArrowRight") return;
    event.preventDefault();
    const nextTab = currentTab === "issues" ? "prs" : "issues";
    setTab(nextTab);
    (nextTab === "issues" ? issuesTabRef : prsTabRef).current?.focus();
  };

  return (
    <div
      ref={panelRef}
      data-embedded-layout={embedded ? (isNarrowEmbedded ? "narrow" : "wide") : undefined}
      className={embedded
        ? "flex min-h-0 h-full w-full flex-col overflow-hidden bg-surface-1"
        : "fixed right-0 top-0 h-full w-[600px] z-40 flex flex-col border-l border-border bg-surface-1 shadow-2xl"}
      style={{ maxWidth: "100vw" }}
    >
      {/* Header */}
      <div className="flex items-center gap-2 px-3 py-2 border-b border-border shrink-0 bg-surface-2">
        <GitPullRequest size={14} className="text-accent" />
        <span className="flex-1 text-label font-semibold text-gray-300">远程仓库</span>
        <button
          onClick={onClose}
          aria-label={embedded ? "返回本地 Git" : "关闭远程仓库"}
          data-auxiliary-initial-focus={embedded ? true : undefined}
          className={embedded
            ? `inline-flex shrink-0 items-center justify-center rounded text-gray-600 transition-colors hover:bg-surface-3 hover:text-gray-300 ${isNarrowEmbedded ? "h-11 w-11" : "h-9 w-9"}`
            : "inline-flex h-9 w-9 shrink-0 items-center justify-center rounded text-gray-600 transition-colors hover:bg-surface-3 hover:text-gray-300"}
          title={embedded ? "返回本地 Git" : "关闭远程仓库"}
        >
          {embedded ? <ChevronLeft size={14} /> : <X size={14} />}
        </button>
      </div>

      {/* Tab bar */}
      <div
        role="tablist"
        aria-label="远程仓库视图"
        className="flex border-b border-border shrink-0 bg-surface-1"
      >
        {(["issues", "prs"] as const).map((t) => (
          <button
            key={t}
            ref={t === "issues" ? issuesTabRef : prsTabRef}
            id={`remote-git-tab-${t}`}
            role="tab"
            aria-selected={tab === t}
            aria-controls={`remote-git-tabpanel-${t}`}
            tabIndex={tab === t ? 0 : -1}
            onClick={() => setTab(t)}
            onKeyDown={(event) => handleTabKeyDown(event, t)}
            className={`${largeTargets ? "h-11" : "h-9"} flex items-center gap-1.5 px-4 text-label border-b-2 transition-colors capitalize ${
              tab === t
                ? "border-accent text-gray-200"
                : "border-transparent text-gray-500 hover:text-gray-300"
            }`}
          >
            {t === "issues" ? <CircleDot size={14} /> : <GitPullRequest size={14} />}
            {t === "prs" ? "拉取请求" : "问题"}
          </button>
        ))}
      </div>

      {/* No remotes configured */}
      {remotes.length === 0 ? (
        <div
          id={activePanelId}
          role="tabpanel"
          aria-labelledby={activeTabId}
          className="flex-1 flex items-center justify-center flex-col gap-2 p-6 text-center"
        >
          <GitPullRequest size={24} className="text-gray-600" />
          <p className="text-label text-gray-600">未配置远程仓库。</p>
          <p className="text-caption text-gray-600">在设置中添加 GitHub 或 GitLab 远程仓库。</p>
        </div>
      ) : (
        <div
          id={activePanelId}
          role="tabpanel"
          aria-labelledby={activeTabId}
          className="flex-1 min-h-0"
        >
          {tab === "issues" ? (
            <IssuesTab remotes={remotes} largeTargets={largeTargets} />
          ) : (
            <PRsTab remotes={remotes} currentBranch={currentBranch} largeTargets={largeTargets} />
          )}
        </div>
      )}
    </div>
  );
}
