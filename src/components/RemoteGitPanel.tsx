// SPDX-License-Identifier: Apache-2.0
import { useEffect, useState } from "react";
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
}

function IssueDetail({ issue, onBack }: IssueDetailProps) {
  return (
    <div className="flex flex-col h-full">
      <div className="flex items-center gap-2 px-3 py-2 border-b border-border shrink-0">
        <button
          onClick={onBack}
          className="p-1 rounded text-gray-600 hover:text-gray-300 hover:bg-surface-3 transition-colors"
        >
          <ChevronLeft size={14} />
        </button>
        <span className="flex-1 text-xs text-gray-400 truncate">
          #{issue.number} {issue.title}
        </span>
        <button
          onClick={() => openUrl(issue.url)}
          className="p-1 rounded text-gray-600 hover:text-gray-300 transition-colors"
          title="在浏览器中打开"
        >
          <ExternalLink size={12} />
        </button>
      </div>

      <div className="flex-1 overflow-y-auto p-4 space-y-3">
        <h2 className="text-sm font-semibold text-gray-200">{issue.title}</h2>

        <div className="flex flex-wrap gap-1.5 text-[10px]">
          <span
            className={`px-1.5 py-0.5 rounded ${
              issue.state === "open"
                ? "bg-green-900 text-green-200"
                : "bg-gray-700 text-gray-400"
            }`}
          >
            {issue.state}
          </span>
          {issue.labels.map((l) => (
            <span
              key={l}
              className="px-1.5 py-0.5 rounded bg-blue-900 text-blue-200 flex items-center gap-0.5"
            >
              <Tag size={8} />
              {l}
            </span>
          ))}
        </div>

        <div className="text-[10px] text-gray-600">
          作者 <span className="text-gray-400">{issue.author}</span> ·{" "}
          {new Date(issue.created_at).toLocaleDateString()}
        </div>

        <div
          className="text-xs text-gray-300 leading-relaxed border border-border rounded p-3 bg-surface-0 prose-sm"
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
}

function NewIssueForm({ remoteId, repo, onCreated, onCancel }: NewIssueFormProps) {
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
        <button onClick={onCancel} className="p-1 rounded text-gray-600 hover:text-gray-300">
          <ChevronLeft size={14} />
        </button>
        <span className="text-xs font-semibold text-gray-300">新建问题</span>
      </div>

      <div className="flex-1 overflow-y-auto p-3 space-y-3">
        <div>
          <label className="block text-[10px] text-gray-600 uppercase tracking-wider mb-1">标题</label>
          <input
            autoFocus
            type="text"
            value={title}
            onChange={(e) => setTitle(e.target.value)}
            placeholder="问题标题…"
            className="w-full bg-surface-3 border border-border rounded px-2 py-1.5 text-xs text-gray-200 placeholder-gray-600 outline-none focus:border-accent/50"
          />
        </div>
        <div>
          <label className="block text-[10px] text-gray-600 uppercase tracking-wider mb-1">描述</label>
          <textarea
            value={body}
            onChange={(e) => setBody(e.target.value)}
            placeholder="描述问题…"
            rows={8}
            className="w-full bg-surface-3 border border-border rounded px-2 py-1.5 text-xs text-gray-200 placeholder-gray-600 outline-none focus:border-accent/50 resize-none"
          />
        </div>
        {error && <p className="text-xs text-red-400">{error}</p>}
      </div>

      <div className="flex gap-2 p-3 border-t border-border shrink-0">
        <button
          onClick={onCancel}
          className="px-3 py-1.5 rounded text-xs text-gray-500 hover:text-gray-300 hover:bg-surface-3 transition-colors"
        >
          取消
        </button>
        <button
          onClick={handleSubmit}
          disabled={busy || !title.trim()}
          className="flex-1 px-3 py-1.5 rounded text-xs bg-accent hover:bg-accent-hover text-white disabled:opacity-50 transition-colors"
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
}

function NewPRForm({ remoteId, repo, currentBranch, onCreated, onCancel }: NewPRFormProps) {
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
        <button onClick={onCancel} className="p-1 rounded text-gray-600 hover:text-gray-300">
          <ChevronLeft size={14} />
        </button>
        <span className="text-xs font-semibold text-gray-300">创建拉取请求</span>
      </div>

      <div className="flex-1 overflow-y-auto p-3 space-y-3">
        <div>
          <label className="block text-[10px] text-gray-600 uppercase tracking-wider mb-1">标题</label>
          <input
            autoFocus
            type="text"
            value={title}
            onChange={(e) => setTitle(e.target.value)}
            placeholder="拉取请求标题…"
            className="w-full bg-surface-3 border border-border rounded px-2 py-1.5 text-xs text-gray-200 placeholder-gray-600 outline-none focus:border-accent/50"
          />
        </div>
        <div className="flex gap-2">
          <div className="flex-1">
            <label className="block text-[10px] text-gray-600 uppercase tracking-wider mb-1">源分支</label>
            <input
              type="text"
              value={head}
              onChange={(e) => setHead(e.target.value)}
              className="w-full bg-surface-3 border border-border rounded px-2 py-1.5 text-xs text-gray-200 outline-none focus:border-accent/50"
            />
          </div>
          <div className="flex-1">
            <label className="block text-[10px] text-gray-600 uppercase tracking-wider mb-1">目标分支</label>
            <input
              type="text"
              value={base}
              onChange={(e) => setBase(e.target.value)}
              className="w-full bg-surface-3 border border-border rounded px-2 py-1.5 text-xs text-gray-200 outline-none focus:border-accent/50"
            />
          </div>
        </div>
        <div>
          <label className="block text-[10px] text-gray-600 uppercase tracking-wider mb-1">描述</label>
          <textarea
            value={body}
            onChange={(e) => setBody(e.target.value)}
            placeholder="描述变更…"
            rows={6}
            className="w-full bg-surface-3 border border-border rounded px-2 py-1.5 text-xs text-gray-200 placeholder-gray-600 outline-none focus:border-accent/50 resize-none"
          />
        </div>
        <label className="flex items-center gap-2 text-xs text-gray-400 cursor-pointer">
          <input
            type="checkbox"
            checked={draft}
            onChange={(e) => setDraft(e.target.checked)}
            className="accent-accent"
          />
          草稿拉取请求
        </label>
        {error && <p className="text-xs text-red-400">{error}</p>}
      </div>

      <div className="flex gap-2 p-3 border-t border-border shrink-0">
        <button
          onClick={onCancel}
          className="px-3 py-1.5 rounded text-xs text-gray-500 hover:text-gray-300 hover:bg-surface-3 transition-colors"
        >
          取消
        </button>
        <button
          onClick={handleSubmit}
          disabled={busy || !title.trim()}
          className="flex-1 px-3 py-1.5 rounded text-xs bg-accent hover:bg-accent-hover text-white disabled:opacity-50 transition-colors"
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
}

function IssuesTab({ remotes }: IssuesTabProps) {
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
        onBack={() => setSelectedIssue(null)}
      />
    );
  }

  if (newIssueOpen) {
    return (
      <NewIssueForm
        remoteId={remoteId}
        repo={repo}
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
            className="flex-1 bg-surface-3 border border-border rounded px-2 py-1 text-xs text-gray-300 outline-none focus:border-accent/50"
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
            className="bg-surface-3 border border-border rounded px-2 py-1 text-xs text-gray-300 outline-none focus:border-accent/50"
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
            className="flex-1 bg-surface-3 border border-border rounded px-2 py-1 text-xs text-gray-200 placeholder-gray-600 outline-none focus:border-accent/50"
          />
          <button
            onClick={handleLoad}
            disabled={!repo || loading}
            className="p-1.5 rounded bg-surface-3 text-gray-400 hover:text-gray-200 disabled:opacity-50 transition-colors"
            title="刷新"
          >
            <RefreshCw size={12} className={loading ? "animate-spin" : ""} />
          </button>
          <button
            onClick={() => setNewIssueOpen(true)}
            disabled={!repo}
            className="p-1.5 rounded bg-surface-3 text-gray-400 hover:text-gray-200 disabled:opacity-50 transition-colors"
            title="新建问题"
          >
            <Plus size={12} />
          </button>
        </div>
      </div>

      {/* Issue list */}
      <div className="flex-1 overflow-y-auto">
        {error && (
          <div className="px-3 py-2 text-xs text-red-400 border-b border-border">{error}</div>
        )}
        {!loading && issues.length === 0 && (
          <div className="px-3 py-3 text-xs text-gray-600">
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
                size={12}
                className={`mt-0.5 shrink-0 ${issue.state === "open" ? "text-green-400" : "text-gray-600"}`}
              />
              <div className="flex-1 min-w-0">
                <div className="flex items-center gap-1.5">
                  <span className="text-[10px] text-gray-600 font-mono shrink-0">#{issue.number}</span>
                  <span className="text-xs text-gray-300 truncate">{issue.title}</span>
                </div>
                {issue.labels.length > 0 && (
                  <div className="flex flex-wrap gap-1 mt-0.5">
                    {issue.labels.slice(0, 3).map((l) => (
                      <span key={l} className="text-[9px] px-1 py-0 rounded bg-surface-3 text-gray-500">
                        {l}
                      </span>
                    ))}
                  </div>
                )}
                <div className="text-[9px] text-gray-700 mt-0.5">
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
}

function PRsTab({ remotes, currentBranch }: PRsTabProps) {
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
      case "open": return "text-green-400";
      case "merged": return "text-purple-400";
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
            className="flex-1 bg-surface-3 border border-border rounded px-2 py-1 text-xs text-gray-300 outline-none focus:border-accent/50"
          >
            {remotes.map((r) => (
              <option key={r.id} value={r.id}>{r.name}</option>
            ))}
          </select>
          <select
            value={stateFilter}
            onChange={(e) => setStateFilter(e.target.value)}
            className="bg-surface-3 border border-border rounded px-2 py-1 text-xs text-gray-300 outline-none focus:border-accent/50"
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
            className="flex-1 bg-surface-3 border border-border rounded px-2 py-1 text-xs text-gray-200 placeholder-gray-600 outline-none focus:border-accent/50"
          />
          <button
            onClick={handleLoad}
            disabled={!repo || loading}
            className="p-1.5 rounded bg-surface-3 text-gray-400 hover:text-gray-200 disabled:opacity-50 transition-colors"
            title="刷新"
          >
            <RefreshCw size={12} className={loading ? "animate-spin" : ""} />
          </button>
          <button
            onClick={() => setNewPROpen(true)}
            disabled={!repo}
            className="p-1.5 rounded bg-surface-3 text-gray-400 hover:text-gray-200 disabled:opacity-50 transition-colors"
            title="创建拉取请求"
          >
            <Plus size={12} />
          </button>
        </div>
      </div>

      {/* PR list */}
      <div className="flex-1 overflow-y-auto">
        {error && (
          <div className="px-3 py-2 text-xs text-red-400 border-b border-border">{error}</div>
        )}
        {!loading && prs.length === 0 && (
          <div className="px-3 py-3 text-xs text-gray-600">
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
              <GitPullRequest size={12} className={`mt-0.5 shrink-0 ${prStateColor(pr.state)}`} />
              <div className="flex-1 min-w-0">
                <div className="flex items-center gap-1.5">
                  <span className="text-[10px] text-gray-600 font-mono shrink-0">#{pr.number}</span>
                  <span className="text-xs text-gray-300 truncate">{pr.title}</span>
                  {pr.draft && (
                    <span className="text-[9px] px-1 rounded bg-surface-3 text-gray-500 shrink-0">草稿</span>
                  )}
                </div>
                <div className="text-[9px] text-gray-700 mt-0.5 font-mono">
                  {pr.head_branch} → {pr.base_branch}
                </div>
                <div className="flex items-center gap-1 mt-0.5">
                  <span className="text-[9px] text-gray-700">
                    {new Date(pr.created_at).toLocaleDateString()}
                  </span>
                  <ExternalLink size={8} className="text-gray-700 opacity-0 group-hover:opacity-100 transition-opacity" />
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
}

export function RemoteGitPanel({ currentBranch, onClose }: RemoteGitPanelProps) {
  const { remotes, loadRemotes } = useGitRemoteStore();
  const [tab, setTab] = useState<"issues" | "prs">("issues");

  useEffect(() => {
    loadRemotes();
  }, [loadRemotes]);

  return (
    <div
      className="fixed right-0 top-0 h-full w-[600px] z-40 flex flex-col border-l border-border bg-surface-1 shadow-2xl"
      style={{ maxWidth: "100vw" }}
    >
      {/* Header */}
      <div className="flex items-center gap-2 px-3 py-2 border-b border-border shrink-0 bg-surface-2">
        <GitPullRequest size={14} className="text-accent" />
        <span className="flex-1 text-xs font-semibold text-gray-300">远程仓库</span>
        <button
          onClick={onClose}
          className="p-1 rounded text-gray-600 hover:text-gray-300 hover:bg-surface-3 transition-colors"
        >
          <X size={14} />
        </button>
      </div>

      {/* Tab bar */}
      <div className="flex border-b border-border shrink-0 bg-surface-1">
        {(["issues", "prs"] as const).map((t) => (
          <button
            key={t}
            onClick={() => setTab(t)}
            className={`flex items-center gap-1.5 px-4 py-2 text-xs border-b-2 transition-colors capitalize ${
              tab === t
                ? "border-accent text-gray-200"
                : "border-transparent text-gray-500 hover:text-gray-300"
            }`}
          >
            {t === "issues" ? <CircleDot size={11} /> : <GitPullRequest size={11} />}
            {t === "prs" ? "拉取请求" : "问题"}
          </button>
        ))}
      </div>

      {/* No remotes configured */}
      {remotes.length === 0 ? (
        <div className="flex-1 flex items-center justify-center flex-col gap-2 p-6 text-center">
          <GitPullRequest size={32} className="text-gray-700" />
          <p className="text-xs text-gray-600">未配置远程仓库。</p>
          <p className="text-[10px] text-gray-700">在设置中添加 GitHub 或 GitLab 远程仓库。</p>
        </div>
      ) : (
        <div className="flex-1 min-h-0">
          {tab === "issues" ? (
            <IssuesTab remotes={remotes} />
          ) : (
            <PRsTab remotes={remotes} currentBranch={currentBranch} />
          )}
        </div>
      )}
    </div>
  );
}
