// SPDX-License-Identifier: Apache-2.0
import { useEffect, useMemo, useState } from "react";
import {
  ArrowLeft,
  Brain,
  Save,
  Check,
  X,
  FolderOpen,
  Sparkles,
  Loader2,
  Lightbulb,
  ShieldAlert,
} from "lucide-react";
import ReactMarkdown from "react-markdown";
import { invoke } from "../../lib/tauri";
import { useChatStore } from "../../stores/chat";
import { useLearningStore, type LearningEvent } from "../../stores/learning";
import { CostDashboardSection } from "../../components/CostDashboardSection";

interface ProfilePageProps {
  onBack: () => void;
  onOpenEvolution?: (cwd?: string) => void;
}

interface ProjectMemory {
  path: string;
  content: string;
  exists: boolean;
}

/**
 * Profile — "我的画像".
 *
 * Three sections:
 *   • 个人偏好         — live key→value, global + per-project, AI-suggested
 *   • 项目记忆 (.md)   — read / edit one project's memory.md at a time
 *   • 学习日志         — live learning events, grouped by session, with
 *                        accept (→ memory.md / preferences) or reject
 *
 * All three are driven by user_preferences / learning_events backend tables
 * and emit `learning_events_updated:{cwd}` so the Workspace right-rail
 * panel stays in sync without polling.
 */
export function ProfilePage({ onBack, onOpenEvolution }: ProfilePageProps) {
  const { sessions, loadSessions } = useChatStore();

  useEffect(() => {
    loadSessions();
  }, []);

  // Pick the most-recently-touched session as the default focus, since
  // it's almost always what the user wants to edit right now.
  const initialCwd = useMemo(() => sessions[0]?.cwd ?? null, [sessions]);
  const [selectedCwd, setSelectedCwd] = useState<string | null>(null);

  useEffect(() => {
    if (!selectedCwd && initialCwd) setSelectedCwd(initialCwd);
  }, [initialCwd]);

  return (
    <div className="h-full flex flex-col bg-surface-0">
      <header className="flex items-center gap-3 px-4 py-2.5 border-b border-border bg-surface-1 shrink-0">
        <button
          onClick={onBack}
          className="p-1 rounded text-gray-500 hover:text-gray-200 hover:bg-surface-3 transition-colors"
          title="返回"
        >
          <ArrowLeft size={14} />
        </button>
        <Brain size={14} className="text-accent" />
        <span className="text-sm font-semibold text-gray-200">我的画像</span>
      </header>

      <div className="flex-1 overflow-y-auto">
        <div className="max-w-3xl mx-auto p-6 space-y-8">

          <PreferencesSection selectedCwd={selectedCwd} />

          <ProjectMemorySection
            sessions={sessions}
            selectedCwd={selectedCwd}
            onSelectCwd={setSelectedCwd}
          />

          <section className="rounded-lg border border-accent/30 bg-accent/5 p-4">
            <div className="flex flex-wrap items-center justify-between gap-3">
              <div className="min-w-0">
                <h2 className="text-xs font-semibold uppercase tracking-wider text-gray-400">进化审查</h2>
                <p className="mt-1 text-xs leading-relaxed text-gray-500">
                  学习候选、人工采纳和端到端作业日志已迁移到独立工作台。
                </p>
              </div>
              <button
                onClick={() => onOpenEvolution?.(selectedCwd ?? undefined)}
                disabled={!onOpenEvolution}
                className="rounded bg-accent px-3 py-1.5 text-xs text-white hover:bg-accent-hover disabled:opacity-50"
              >
                前往进化审查
              </button>
            </div>
          </section>

          <SelfImprovementSection />

          <ToolGateSection />

          <CostDashboardSection />

        </div>
      </div>
    </div>
  );
}

// ─────────────────────────────────────────────────────────────────────────────
// PreferencesSection — live key→value preferences from user_preferences table
// ─────────────────────────────────────────────────────────────────────────────

interface UserPreference {
  cwd: string;
  key: string;
  value: string;
  source: "user" | "ai" | "default";
  updated_at: string;
}

// Friendly labels + hints for seeded keys. Unknown keys (added later by
// AI suggestions or user) fall back to the raw key as the label.
const PREF_LABELS: Record<string, { label: string; hint: string }> = {
  autonomy_level:      { label: "自主程度", hint: "AI 多大程度上自主操作不询问" },
  communication_style: { label: "沟通风格", hint: "AI 回复的详略偏好" },
  testing_habit:       { label: "测试习惯", hint: "AI 主动加测试的时机" },
  code_style:          { label: "代码风格", hint: "格式 / 命名 / 习惯偏好" },
};

const SOURCE_BADGE: Record<UserPreference["source"], { text: string; cls: string }> = {
  user:    { text: "我设的",  cls: "bg-accent/15 text-accent" },
  ai:      { text: "AI 学的", cls: "bg-purple-500/15 text-purple-700 dark:text-purple-300" },
  default: { text: "默认",    cls: "bg-surface-3 text-gray-500" },
};

/** Sentinel cwd value matching `commands::preferences::GLOBAL_CWD` on the
 *  backend. Stored side-by-side with project rows in user_preferences;
 *  the scheduler merges them with project-overrides-global semantics. */
const GLOBAL_CWD = "_global_";

function PreferencesSection({ selectedCwd }: { selectedCwd: string | null }) {
  const [scope, setScope] = useState<"global" | "project">("global");
  const [prefs, setPrefs] = useState<UserPreference[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // Resolve which cwd to query based on selected scope. When no project is
  // open, project tab is disabled — global is always available.
  const activeCwd = scope === "global" ? GLOBAL_CWD : selectedCwd;

  const reload = async (cwd: string) => {
    setLoading(true);
    setError(null);
    try {
      const list = await invoke<UserPreference[]>("list_user_preferences", { cwd });
      setPrefs(list);
    } catch (e) {
      setError(String(e));
    } finally {
      setLoading(false);
    }
  };

  useEffect(() => {
    if (activeCwd) reload(activeCwd);
  }, [activeCwd]);

  const handleUpdate = async (key: string, value: string) => {
    if (!activeCwd) return;
    try {
      await invoke("upsert_user_preference", {
        cwd: activeCwd,
        key,
        value,
        source: "user",
      });
      // Optimistic local patch; refresh would also work but feels laggy.
      setPrefs((prev) =>
        prev.map((p) =>
          p.key === key
            ? { ...p, value, source: "user" as const, updated_at: new Date().toISOString() }
            : p,
        ),
      );
    } catch (e) {
      setError(String(e));
    }
  };

  const projectDisabled = !selectedCwd;

  return (
    <section>
      <div className="flex items-center justify-between mb-3">
        <h2 className="text-xs font-semibold text-gray-400 uppercase tracking-wider">
          个人偏好
        </h2>
        {/* Scope tabs — global vs current project. */}
        <div className="flex items-center rounded border border-border overflow-hidden text-[10px]">
          <button
            onClick={() => setScope("global")}
            className={`px-2 py-0.5 transition-colors ${
              scope === "global"
                ? "bg-surface-3 text-accent"
                : "text-gray-500 hover:text-gray-300 hover:bg-surface-3"
            }`}
            title="所有项目共享的偏好"
          >
            全局
          </button>
          <button
            onClick={() => !projectDisabled && setScope("project")}
            disabled={projectDisabled}
            className={`px-2 py-0.5 transition-colors ${
              scope === "project"
                ? "bg-surface-3 text-accent"
                : "text-gray-500 hover:text-gray-300 hover:bg-surface-3 disabled:opacity-40 disabled:cursor-not-allowed"
            }`}
            title={projectDisabled ? "选一个项目后才能设置项目级偏好" : "覆盖全局的项目级偏好"}
          >
            当前项目
          </button>
        </div>
      </div>
      {scope === "project" && !selectedCwd ? (
        <p className="text-xs text-gray-500 text-center py-6">选一个项目以查看项目偏好</p>
      ) : loading ? (
        <p className="text-xs text-gray-500 text-center py-6">加载中...</p>
      ) : (
        <div className="rounded-lg border border-border bg-surface-1 divide-y divide-border">
          {prefs.map((p) => (
            <PrefRow
              key={p.key}
              pref={p}
              label={PREF_LABELS[p.key]?.label ?? p.key}
              hint={PREF_LABELS[p.key]?.hint ?? "AI 学到的偏好"}
              onUpdate={(v) => handleUpdate(p.key, v)}
            />
          ))}
        </div>
      )}
      <p className="mt-2 text-[11px] text-gray-500">
        {scope === "global"
          ? "全局偏好对所有项目生效。当前项目可在「当前项目」标签下覆盖。"
          : "项目偏好覆盖同 key 的全局偏好；其他 key 仍继承全局。"}
      </p>
      {error && <p className="mt-2 text-xs text-red-700 dark:text-red-300">{error}</p>}
    </section>
  );
}

function PrefRow({
  pref,
  label,
  hint,
  onUpdate,
}: {
  pref: UserPreference;
  label: string;
  hint: string;
  onUpdate: (v: string) => void;
}) {
  const [draft, setDraft] = useState(pref.value);
  const [editing, setEditing] = useState(false);

  useEffect(() => { setDraft(pref.value); }, [pref.value]);

  const badge = SOURCE_BADGE[pref.source];

  const commit = () => {
    if (draft !== pref.value) onUpdate(draft);
    setEditing(false);
  };

  return (
    <div className="flex items-start gap-4 px-4 py-3">
      <div className="w-24 shrink-0">
        <div className="text-xs font-medium text-gray-300">{label}</div>
        <span className={`mt-1 inline-block text-[9px] px-1.5 py-0.5 rounded ${badge.cls}`}>
          {badge.text}
        </span>
      </div>
      <div className="flex-1 min-w-0">
        {editing ? (
          <input
            autoFocus
            value={draft}
            onChange={(e) => setDraft(e.target.value)}
            onBlur={commit}
            onKeyDown={(e) => {
              if (e.key === "Enter") commit();
              else if (e.key === "Escape") {
                setDraft(pref.value);
                setEditing(false);
              }
            }}
            className="w-full bg-surface-3 border border-border rounded px-2 py-1 text-sm text-gray-200 outline-none focus:border-accent"
          />
        ) : (
          <button
            onClick={() => setEditing(true)}
            className="text-sm text-gray-200 hover:text-accent text-left w-full"
          >
            {pref.value || <span className="text-gray-500 italic">（点击设置）</span>}
          </button>
        )}
        <div className="text-[11px] text-gray-500 mt-0.5">{hint}</div>
      </div>
    </div>
  );
}

// ─────────────────────────────────────────────────────────────────────────────
// ProjectMemorySection — read / edit a project's memory.md
// ─────────────────────────────────────────────────────────────────────────────

interface SessionLite {
  id: string;
  cwd: string;
  title?: string | null;
}

function ProjectMemorySection({
  sessions,
  selectedCwd,
  onSelectCwd,
}: {
  sessions: SessionLite[];
  selectedCwd: string | null;
  onSelectCwd: (cwd: string) => void;
}) {
  const [memory, setMemory] = useState<ProjectMemory | null>(null);
  const [draft, setDraft] = useState("");
  const [loading, setLoading] = useState(false);
  const [saving, setSaving] = useState(false);
  const [savedAt, setSavedAt] = useState(0);
  const [error, setError] = useState<string | null>(null);

  // Dedup sessions by cwd (one repo can have multiple chat sessions)
  const projects = useMemo(() => {
    const seen = new Set<string>();
    const out: SessionLite[] = [];
    for (const s of sessions) {
      if (!s.cwd || seen.has(s.cwd)) continue;
      seen.add(s.cwd);
      out.push(s);
    }
    return out;
  }, [sessions]);

  useEffect(() => {
    if (!selectedCwd) return;
    setLoading(true);
    setError(null);
    invoke<ProjectMemory>("read_project_memory", { cwd: selectedCwd })
      .then((m) => {
        setMemory(m);
        setDraft(m.content);
      })
      .catch((e) => setError(String(e)))
      .finally(() => setLoading(false));
  }, [selectedCwd]);

  const dirty = memory !== null && draft !== memory.content;

  const save = async () => {
    if (!selectedCwd) return;
    setSaving(true);
    setError(null);
    try {
      const updated = await invoke<ProjectMemory>("write_project_memory", {
        cwd: selectedCwd,
        content: draft,
      });
      setMemory(updated);
      setSavedAt(Date.now());
    } catch (e) {
      setError(String(e));
    } finally {
      setSaving(false);
    }
  };

  return (
    <section>
      <h2 className="text-xs font-semibold text-gray-400 uppercase tracking-wider mb-3">
        项目记忆
      </h2>
      <div className="rounded-lg border border-border bg-surface-1 overflow-hidden">

        {/* Project picker */}
        <div className="px-4 py-3 border-b border-border bg-surface-2 flex items-center gap-2">
          <FolderOpen size={12} className="text-gray-500 shrink-0" />
          <select
            value={selectedCwd ?? ""}
            onChange={(e) => onSelectCwd(e.target.value)}
            className="flex-1 bg-surface-3 border border-border rounded px-2 py-1 text-xs text-gray-200 outline-none focus:border-accent font-mono"
          >
            {projects.length === 0 && <option value="">（暂无项目）</option>}
            {projects.map((s) => (
              <option key={s.id} value={s.cwd}>
                {s.cwd}
              </option>
            ))}
          </select>
        </div>

        {/* Editor */}
        <div className="p-4">
          {loading ? (
            <p className="text-xs text-gray-500 text-center py-8">加载中...</p>
          ) : !selectedCwd ? (
            <p className="text-xs text-gray-500 text-center py-8">
              选一个项目以查看记忆
            </p>
          ) : (
            <>
              <textarea
                value={draft}
                onChange={(e) => setDraft(e.target.value)}
                rows={14}
                placeholder="还没有内容。试着写一些关于这个项目的事实，例如：&#10;- 这个项目使用 pnpm，不用 npm&#10;- API 错误码格式遵循 RFC 7807&#10;- 模型代码集中在 src/models/ 下"
                className="w-full bg-surface-2 border border-border rounded px-3 py-2 text-sm text-gray-200 font-mono leading-relaxed outline-none focus:border-accent resize-y"
              />
              <p className="mt-2 text-[11px] text-gray-500">
                这份记忆会在该项目每次对话开始时自动注入到 AI 的系统提示。
                {memory && (
                  <>
                    {" "}文件位置：<span className="font-mono text-gray-400">{memory.path}</span>
                  </>
                )}
              </p>
            </>
          )}

          {error && (
            <p className="mt-2 text-xs text-red-700 dark:text-red-300">{error}</p>
          )}
        </div>

        {/* Footer */}
        {selectedCwd && (
          <div className="flex items-center justify-end gap-2 px-4 py-3 border-t border-border bg-surface-2">
            {savedAt > 0 && !dirty && (
              <span className="text-[11px] text-green-700 dark:text-green-400 flex items-center gap-1">
                <Check size={11} /> 已保存
              </span>
            )}
            <button
              onClick={save}
              disabled={!dirty || saving}
              className="flex items-center gap-1.5 px-3 py-1.5 rounded bg-accent hover:bg-accent-hover text-white text-xs disabled:opacity-40 disabled:cursor-not-allowed"
            >
              <Save size={11} />
              {saving ? "保存中..." : "保存记忆"}
            </button>
          </div>
        )}
      </div>
    </section>
  );
}

// ─────────────────────────────────────────────────────────────────────────────
// LearningLogSection — live learning events grouped by session
// ─────────────────────────────────────────────────────────────────────────────
//
// Pulls from the shared useLearningStore so this view stays in sync with the
// Workspace right-rail panel — accept/reject in either place updates both.
// Backend emits `learning_events_updated:{cwd}` after run_postmortem /
// accept / reject so we don't poll.
//
// Grouping: pending events are flat (you act on them) but decided events
// are grouped by session_id so historical exploration scales past a few
// dozen runs without becoming a wall of cards.

// Stable empty array for the selector fallback — see ConnectorsColumn
// note in WorkspacePage.tsx; same Zustand-referential-equality trap.
const EMPTY_LEARNING_EVENTS: LearningEvent[] = [];

export function LearningLogSection({ selectedCwd }: { selectedCwd: string | null }) {
  const events = useLearningStore(
    (s) => (selectedCwd ? s.events[selectedCwd] ?? EMPTY_LEARNING_EVENTS : EMPTY_LEARNING_EVENTS),
  );
  const loading = useLearningStore((s) => (selectedCwd ? s.loading[selectedCwd] ?? false : false));
  const load = useLearningStore((s) => s.load);
  const subscribe = useLearningStore((s) => s.subscribe);
  const accept = useLearningStore((s) => s.accept);
  const reject = useLearningStore((s) => s.reject);
  const mine = useLearningStore((s) => s.mine);
  const [busyId, setBusyId] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [mining, setMining] = useState(false);
  const [filter, setFilter] = useState<"all" | "memory" | "preference" | "pattern">("all");

  useEffect(() => {
    if (!selectedCwd) return;
    void load(selectedCwd);
    let off: (() => void) | undefined;
    subscribe(selectedCwd).then((u) => { off = u; });
    return () => { off?.(); };
  }, [selectedCwd]);

  const handleAccept = async (id: string) => {
    if (!selectedCwd) return;
    setBusyId(id);
    setError(null);
    try {
      await accept(id, selectedCwd);
    } catch (e) {
      setError(String(e));
    } finally {
      setBusyId(null);
    }
  };

  const handleReject = async (id: string) => {
    if (!selectedCwd) return;
    setBusyId(id);
    setError(null);
    try {
      await reject(id, selectedCwd);
    } catch (e) {
      setError(String(e));
    } finally {
      setBusyId(null);
    }
  };

  const handleMine = async () => {
    if (!selectedCwd || mining) return;
    setMining(true);
    setError(null);
    try {
      await mine(selectedCwd);
    } catch (e) {
      setError(String(e));
    } finally {
      setMining(false);
    }
  };

  const filtered = filter === "all" ? events : events.filter((e) => e.kind === filter);
  const pending = filtered.filter((e) => e.status === "pending");
  const decided = filtered.filter((e) => e.status !== "pending");
  // Group decided by session_id. Order: most-recent session first
  // (by max created_at within the group).
  const decidedBySession: Record<string, LearningEvent[]> = {};
  for (const e of decided) {
    (decidedBySession[e.session_id] ??= []).push(e);
  }
  const sessionGroups = Object.entries(decidedBySession).sort(
    ([, a], [, b]) =>
      (b[0]?.decided_at ?? b[0]?.created_at ?? "").localeCompare(
        a[0]?.decided_at ?? a[0]?.created_at ?? "",
      ),
  );

  return (
    <section>
      <div className="flex items-center justify-between mb-3">
        <h2 className="text-xs font-semibold text-gray-400 uppercase tracking-wider">
          学习日志
          {pending.length > 0 && (
            <span className="ml-2 text-[10px] font-normal text-accent normal-case">
              {pending.length} 条待审
            </span>
          )}
        </h2>
        <div className="flex items-center gap-2">
          <button
            onClick={handleMine}
            disabled={!selectedCwd || mining}
            title="跨会话分析：从多次会话的工具失败 / 重试 / 学习接受率里挖出反复出现的模式"
            className="flex items-center gap-1 rounded border border-border px-2 py-0.5 text-[10px] text-gray-300 transition-colors hover:bg-surface-3 disabled:opacity-50"
          >
            {mining ? (
              <Loader2 size={11} className="animate-spin" />
            ) : (
              <Sparkles size={11} className="text-accent" />
            )}
            分析跨会话模式
          </button>
          <div className="flex items-center rounded border border-border overflow-hidden text-[10px]">
            {(["all", "memory", "preference", "pattern"] as const).map((f) => (
              <button
                key={f}
                onClick={() => setFilter(f)}
                className={`px-2 py-0.5 transition-colors ${
                  filter === f
                    ? "bg-surface-3 text-accent"
                    : "text-gray-500 hover:text-gray-300 hover:bg-surface-3"
                }`}
              >
                {f === "all" ? "全部" : f === "memory" ? "记忆" : f === "preference" ? "偏好" : "模式"}
              </button>
            ))}
          </div>
        </div>
      </div>

      {!selectedCwd ? (
        <p className="text-xs text-gray-500 text-center py-6">选一个项目以查看学习记录</p>
      ) : loading ? (
        <p className="text-xs text-gray-500 text-center py-6 flex items-center justify-center gap-2">
          <Loader2 size={12} className="animate-spin" /> 加载中...
        </p>
      ) : filtered.length === 0 ? (
        <div className="rounded-lg border border-dashed border-border bg-surface-1 px-6 py-10 text-center">
          <Sparkles size={20} className="text-gray-600 mx-auto mb-3" />
          <p className="text-sm text-gray-400 font-medium mb-1">
            {events.length === 0
              ? "暂无学习记录"
              : `没有「${filter === "memory" ? "记忆" : filter === "preference" ? "偏好" : "模式"}」类型的记录`}
          </p>
          <p className="text-xs text-gray-500 max-w-md mx-auto leading-relaxed">
            每次任务 / 聊天 session 结束后，AI 自动总结观察到的事实，
            出现在这里等你审核。审核通过的会写入项目记忆或偏好，影响未来对话。
          </p>
        </div>
      ) : (
        <div className="space-y-3">
          {pending.length > 0 && (
            <div className="space-y-2">
              {pending.map((e) => (
                <LearningEventCard
                  key={e.id}
                  event={e}
                  busy={busyId === e.id}
                  onAccept={() => handleAccept(e.id)}
                  onReject={() => handleReject(e.id)}
                />
              ))}
            </div>
          )}

          {sessionGroups.length > 0 && (
            <details className="rounded-lg border border-border bg-surface-1">
              <summary className="px-4 py-2 text-xs text-gray-500 cursor-pointer hover:text-gray-300 select-none">
                历史决定 ({decided.length} · {sessionGroups.length} 个 session)
              </summary>
              <div className="border-t border-border divide-y divide-border">
                {sessionGroups.map(([sid, list]) => (
                  <details key={sid} className="px-4 py-2 group" open>
                    <summary className="text-[11px] text-gray-500 cursor-pointer hover:text-gray-300 select-none flex items-center gap-2">
                      <span className="font-mono text-gray-600">{sid.slice(0, 8)}</span>
                      <span className="text-gray-700">·</span>
                      <span>{list.length} 条</span>
                      <span className="ml-auto text-[10px] text-gray-600">
                        {(list[0]?.decided_at ?? list[0]?.created_at ?? "").slice(0, 10)}
                      </span>
                    </summary>
                    <ul className="mt-1.5 space-y-1">
                      {list.map((e) => (
                        <li key={e.id} className="text-[11px] pl-3">
                          <span
                            className={
                              e.status === "accepted"
                                ? "text-green-700 dark:text-green-400 mr-2"
                                : "text-gray-500 mr-2"
                            }
                          >
                            {e.status === "accepted" ? "✓" : "✕"}
                          </span>
                          <span className="text-[9px] text-gray-600 mr-1">
                            {e.kind === "preference" ? "偏好" : "记忆"}
                          </span>
                          <span className="text-gray-400">{e.observation}</span>
                        </li>
                      ))}
                    </ul>
                  </details>
                ))}
              </div>
            </details>
          )}
        </div>
      )}

      {error && (
        <p className="mt-2 text-xs text-red-700 dark:text-red-300">{error}</p>
      )}
    </section>
  );
}

// ─────────────────────────────────────────────────────────────────────────────
// SelfImprovementSection — read-only self-improvement proposal (self-evolution P4)
// ─────────────────────────────────────────────────────────────────────────────
//
// Surfaces the `self_improvement_proposal` command: a GLOBAL, read-only
// aggregation of recurring friction (flaky tools, retry-prone failures) across
// ALL projects, rendered as a markdown 改进提案 for the human. By contract it
// writes no code, opens no PR, ships nothing — the system proposes, the human
// disposes. This is the only user-facing surface of P4; the autonomous
// implement→verify→PR loop stays human-gated (see docs/self-evolution/P4).

export function SelfImprovementSection() {
  const [proposal, setProposal] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const generate = async () => {
    if (loading) return;
    setLoading(true);
    setError(null);
    try {
      // Global aggregation — no cwd; reuses P1's detectors across all projects.
      const md = await invoke<string>("self_improvement_proposal");
      setProposal(md);
    } catch (e) {
      setError(String(e));
    } finally {
      setLoading(false);
    }
  };

  return (
    <section>
      <div className="flex items-center justify-between mb-3">
        <h2 className="text-xs font-semibold text-gray-400 uppercase tracking-wider">
          自我改进提案
        </h2>
        <button
          onClick={generate}
          disabled={loading}
          title="只读分析:跨所有项目聚合反复出现的摩擦(工具失败 / 反复重试),生成一份给你看的改进提案。不改任何代码、不开 PR、不发版。"
          className="flex items-center gap-1 rounded border border-border px-2 py-0.5 text-[10px] text-gray-300 transition-colors hover:bg-surface-3 disabled:opacity-50"
        >
          {loading ? (
            <Loader2 size={11} className="animate-spin" />
          ) : (
            <Lightbulb size={11} className="text-accent" />
          )}
          {proposal === null ? "生成改进提案" : "重新生成"}
        </button>
      </div>

      {error && (
        <p className="mb-2 text-xs text-red-700 dark:text-red-300">{error}</p>
      )}

      {proposal === null ? (
        <div className="rounded-lg border border-dashed border-border bg-surface-1 px-6 py-10 text-center">
          <Lightbulb size={20} className="text-gray-600 mx-auto mb-3" />
          <p className="text-sm text-gray-400 font-medium mb-1">还没有生成提案</p>
          <p className="text-xs text-gray-500 max-w-md mx-auto leading-relaxed">
            点「生成改进提案」,系统会<strong className="text-gray-400">只读</strong>聚合你跨所有项目的反复摩擦点
            (常失败的工具、反复重试的步骤),给出一份改进建议。它只是
            <strong className="text-gray-400">提议</strong> —— 改不改、怎么改都由你定,绝不自动动代码或发版。
          </p>
        </div>
      ) : (
        <div className="rounded-lg border border-border bg-surface-1 p-4">
          <div className="prose dark:prose-invert prose-sm max-w-none [&>*:first-child]:mt-0 [&>*:last-child]:mb-0">
            <ReactMarkdown>{proposal}</ReactMarkdown>
          </div>
        </div>
      )}
    </section>
  );
}

// ─────────────────────────────────────────────────────────────────────────────
// ToolGateSection — flaky-tool gating proposals (self-evolution P3 tool-policy)
// ─────────────────────────────────────────────────────────────────────────────
//
// P1 mines which tools fail a lot; this proposes moving a flaky, currently
// auto-allowed tool from `allow` to `ask` so the agent confirms before running
// it. Read-only scan; the gate applies only when the human clicks 启用门控.
// Rides the existing decide_permission — see docs/self-evolution/P3-tool-policy.md.

interface ToolGateProposal {
  tool: string;
  total: number;
  errors: number;
  rate: number;
  observation: string;
}

export function ToolGateSection() {
  const [proposals, setProposals] = useState<ToolGateProposal[] | null>(null);
  const [loading, setLoading] = useState(false);
  const [busyTool, setBusyTool] = useState<string | null>(null);
  const [gated, setGated] = useState<string[]>([]);
  const [error, setError] = useState<string | null>(null);

  const scan = async () => {
    if (loading) return;
    setLoading(true);
    setError(null);
    try {
      setProposals(await invoke<ToolGateProposal[]>("propose_tool_gates"));
    } catch (e) {
      setError(String(e));
    } finally {
      setLoading(false);
    }
  };

  const gate = async (tool: string) => {
    setBusyTool(tool);
    setError(null);
    try {
      // The human gate: only now does the policy change (allow → ask).
      await invoke("apply_tool_gate", { tool });
      setGated((g) => [...g, tool]);
      setProposals((ps) => ps?.filter((p) => p.tool !== tool) ?? null);
    } catch (e) {
      setError(String(e));
    } finally {
      setBusyTool(null);
    }
  };

  return (
    <section>
      <div className="flex items-center justify-between mb-3">
        <h2 className="text-xs font-semibold text-gray-400 uppercase tracking-wider">
          工具门控建议
        </h2>
        <button
          onClick={scan}
          disabled={loading}
          title="只读扫描:从跨会话调用记录里找出反复失败、却仍在自动放行的工具,建议改成「运行前先确认」。启用与否由你定,只会更谨慎、随时可在设置里改回。"
          className="flex items-center gap-1 rounded border border-border px-2 py-0.5 text-[10px] text-gray-300 transition-colors hover:bg-surface-3 disabled:opacity-50"
        >
          {loading ? (
            <Loader2 size={11} className="animate-spin" />
          ) : (
            <ShieldAlert size={11} className="text-accent" />
          )}
          扫描易错工具
        </button>
      </div>

      {error && (
        <p className="mb-2 text-xs text-red-700 dark:text-red-300">{error}</p>
      )}

      {proposals === null ? (
        <div className="rounded-lg border border-dashed border-border bg-surface-1 px-6 py-10 text-center">
          <ShieldAlert size={20} className="text-gray-600 mx-auto mb-3" />
          <p className="text-sm text-gray-400 font-medium mb-1">还没有扫描</p>
          <p className="text-xs text-gray-500 max-w-md mx-auto leading-relaxed">
            点「扫描易错工具」,系统会找出反复失败、却仍自动放行的工具,建议给它们加一道运行前确认。
            纯<strong className="text-gray-400">建议</strong> —— 启用只会让工具更谨慎(自动→先问),随时可在设置里改回。
          </p>
        </div>
      ) : proposals.length === 0 ? (
        <div className="rounded-lg border border-border bg-surface-1 px-6 py-8 text-center">
          <Check size={18} className="text-green-600 mx-auto mb-2" />
          <p className="text-xs text-gray-400">
            {gated.length > 0
              ? `已门控:${gated.join("、")}——已写入设置的「询问」清单,随时可改回。`
              : "没发现需要门控的工具——自动放行的工具最近都挺稳。"}
          </p>
        </div>
      ) : (
        <div className="space-y-2">
          {proposals.map((p) => (
            <div
              key={p.tool}
              className="rounded-lg border border-amber-500/40 bg-amber-500/5 p-4"
            >
              <div className="flex items-start gap-2">
                <ShieldAlert size={12} className="text-amber-500 mt-0.5 shrink-0" />
                <div className="flex-1 min-w-0">
                  <p className="text-xs text-gray-300 leading-relaxed">
                    <span className="font-mono text-amber-700 dark:text-amber-300">
                      {p.tool}
                    </span>{" "}
                    最近 {p.total} 次调用失败 {p.errors} 次
                    <span className="text-gray-500"> ({p.rate}%)</span>,但仍在自动放行。
                  </p>
                  <p className="text-[11px] text-gray-500 mt-1">
                    建议改为「运行前先确认」——只影响这一个工具,随时可在设置里改回。
                  </p>
                </div>
                <button
                  onClick={() => gate(p.tool)}
                  disabled={busyTool === p.tool}
                  className="flex items-center gap-1 px-3 py-1 rounded bg-accent hover:bg-accent-hover text-white text-xs disabled:opacity-40 shrink-0"
                >
                  {busyTool === p.tool ? (
                    <Loader2 size={11} className="animate-spin" />
                  ) : (
                    <ShieldAlert size={11} />
                  )}
                  启用门控
                </button>
              </div>
            </div>
          ))}
          {gated.length > 0 && (
            <p className="text-[11px] text-green-700 dark:text-green-400 flex items-center gap-1 pl-1">
              <Check size={11} /> 已门控:{gated.join("、")}(已写入设置的「询问」清单)
            </p>
          )}
        </div>
      )}
    </section>
  );
}

function patternEvidenceSummary(event: LearningEvent): string {
  if (event.kind !== "pattern") return "";
  try {
    const evidence = JSON.parse(event.evidence_json) as Record<string, unknown>;
    const detector = String(evidence.detector ?? "");
    const hasSessionEvidence =
      evidence.support_unit === "sessions" || typeof evidence.session_count === "number";
    if (detector === "tool_reliability" && hasSessionEvidence) {
      return `${Number(evidence.session_count ?? event.support_count)} 个 session · ${Number(evidence.total_calls ?? evidence.total ?? 0)} 次调用 · ${Number(evidence.errors ?? 0)} 次错误 · ${Number(evidence.rate ?? 0)}%`;
    }
    if (detector === "retry_prone" && hasSessionEvidence) {
      return `${Number(evidence.session_count ?? event.support_count)} 个 session · ${Number(evidence.task_count ?? 0)} 个重试任务`;
    }
    if (detector === "learning_calibration") {
      return `${Number(evidence.decision_count ?? evidence.decided ?? event.support_count)} 次人工决定`;
    }
  } catch {
    // Legacy rows may contain malformed evidence; keep the review surface
    // usable and state the generic support count instead of hiding the card.
  }
  return `${event.support_count} 条支持证据`;
}

function patternSupportLabel(event: LearningEvent): string {
  if (event.kind !== "pattern") return "";
  try {
    const evidence = JSON.parse(event.evidence_json) as Record<string, unknown>;
    if (evidence.support_unit === "sessions") {
      return `${event.support_count} 个 session`;
    }
    if (evidence.support_unit === "decisions") {
      return `${event.support_count} 次决策`;
    }
  } catch {
    // Fall through to the neutral legacy label.
  }
  return `${event.support_count} 条证据`;
}

export function LearningEventCard({
  event,
  busy,
  onAccept,
  onReject,
}: {
  event: LearningEvent;
  busy: boolean;
  onAccept: () => void;
  onReject: () => void;
}) {
  const isPref = event.kind === "preference";
  const isPattern = event.kind === "pattern";
  const evidenceSummary = patternEvidenceSummary(event);
  const supportLabel = patternSupportLabel(event);
  return (
    <div className="rounded-lg border border-accent/40 bg-accent/5 p-4">
      <div className="flex items-start gap-2 mb-2">
        <Sparkles size={12} className="text-accent mt-0.5 shrink-0" />
        <p className="text-xs text-gray-300 leading-relaxed flex-1">{event.observation}</p>
        <span
          className={`text-[9px] px-1.5 py-0.5 rounded shrink-0 ${
            isPref
              ? "bg-purple-500/15 text-purple-700 dark:text-purple-300"
              : isPattern
                ? "bg-emerald-500/15 text-emerald-700 dark:text-emerald-300"
                : "bg-accent/15 text-accent"
          }`}
          title={
            isPref
              ? "采纳后写入「个人偏好」表"
              : isPattern
                ? `跨会话挖掘的模式（${evidenceSummary}），采纳后追加到 memory.md`
                : "采纳后追加到 memory.md"
          }
        >
          {isPref ? "偏好" : isPattern ? `模式 · ${supportLabel}` : "记忆"}
        </span>
      </div>
      <div className="rounded bg-surface-2 border border-border px-3 py-2 mb-3">
        <div className="text-[10px] uppercase tracking-wider text-gray-500 mb-1">
          {isPref ? "建议更新偏好" : isPattern ? "跨会话模式 → 写入记忆" : "建议写入记忆"}
        </div>
        <p className="text-[12px] text-gray-200 font-mono leading-relaxed">{event.suggestion}</p>
        {isPref && event.pref_key && (
          <p className="mt-1 text-[10px] text-gray-500 font-mono">
            → {event.pref_key} = <span className="text-accent">{event.pref_value ?? ""}</span>
          </p>
        )}
        {isPattern && evidenceSummary && (
          <p className="mt-1 text-[10px] text-gray-500">
            证据：{evidenceSummary}
          </p>
        )}
      </div>
      <div className="flex justify-end gap-2">
        <button
          onClick={onReject}
          disabled={busy}
          className="flex items-center gap-1 px-3 py-1 rounded text-xs text-gray-400 hover:bg-surface-3 disabled:opacity-40"
        >
          <X size={11} /> 拒绝
        </button>
        <button
          onClick={onAccept}
          disabled={busy}
          className="flex items-center gap-1 px-3 py-1 rounded bg-accent hover:bg-accent-hover text-white text-xs disabled:opacity-40"
        >
          {busy ? <Loader2 size={11} className="animate-spin" /> : <Check size={11} />}
          {isPref ? "采纳并更新偏好" : "采纳并写入记忆"}
        </button>
      </div>
    </div>
  );
}
