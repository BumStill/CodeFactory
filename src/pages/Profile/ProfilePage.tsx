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
} from "lucide-react";
import { invoke } from "../../lib/tauri";
import { useChatStore } from "../../stores/chat";
import { useLearningStore, type LearningEvent } from "../../stores/learning";

interface ProfilePageProps {
  onBack: () => void;
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
export function ProfilePage({ onBack }: ProfilePageProps) {
  const { sessions, loadSessions } = useChatStore();

  useEffect(() => {
    loadSessions();
  }, []);

  // Pick the most-recently-touched session as the default focus, since
  // it's almost always what the user wants to edit right now.
  const initialCwd = useMemo(() => sessions[0]?.cwd ?? null, [sessions.length]);
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

          <LearningLogSection selectedCwd={selectedCwd} />

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

function LearningLogSection({ selectedCwd }: { selectedCwd: string | null }) {
  const events = useLearningStore(
    (s) => (selectedCwd ? s.events[selectedCwd] ?? EMPTY_LEARNING_EVENTS : EMPTY_LEARNING_EVENTS),
  );
  const loading = useLearningStore((s) => (selectedCwd ? s.loading[selectedCwd] ?? false : false));
  const load = useLearningStore((s) => s.load);
  const subscribe = useLearningStore((s) => s.subscribe);
  const accept = useLearningStore((s) => s.accept);
  const reject = useLearningStore((s) => s.reject);
  const [busyId, setBusyId] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [filter, setFilter] = useState<"all" | "memory" | "preference">("all");

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
        <div className="flex items-center rounded border border-border overflow-hidden text-[10px]">
          {(["all", "memory", "preference"] as const).map((f) => (
            <button
              key={f}
              onClick={() => setFilter(f)}
              className={`px-2 py-0.5 transition-colors ${
                filter === f
                  ? "bg-surface-3 text-accent"
                  : "text-gray-500 hover:text-gray-300 hover:bg-surface-3"
              }`}
            >
              {f === "all" ? "全部" : f === "memory" ? "记忆" : "偏好"}
            </button>
          ))}
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
            {events.length === 0 ? "暂无学习记录" : `没有「${filter === "memory" ? "记忆" : "偏好"}」类型的记录`}
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

function LearningEventCard({
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
  return (
    <div className="rounded-lg border border-accent/40 bg-accent/5 p-4">
      <div className="flex items-start gap-2 mb-2">
        <Sparkles size={12} className="text-accent mt-0.5 shrink-0" />
        <p className="text-xs text-gray-300 leading-relaxed flex-1">{event.observation}</p>
        <span
          className={`text-[9px] px-1.5 py-0.5 rounded shrink-0 ${
            isPref
              ? "bg-purple-500/15 text-purple-700 dark:text-purple-300"
              : "bg-accent/15 text-accent"
          }`}
          title={isPref ? "采纳后写入「个人偏好」表" : "采纳后追加到 memory.md"}
        >
          {isPref ? "偏好" : "记忆"}
        </span>
      </div>
      <div className="rounded bg-surface-2 border border-border px-3 py-2 mb-3">
        <div className="text-[10px] uppercase tracking-wider text-gray-500 mb-1">
          {isPref ? "建议更新偏好" : "建议写入记忆"}
        </div>
        <p className="text-[12px] text-gray-200 font-mono leading-relaxed">{event.suggestion}</p>
        {isPref && event.pref_key && (
          <p className="mt-1 text-[10px] text-gray-500 font-mono">
            → {event.pref_key} = <span className="text-accent">{event.pref_value ?? ""}</span>
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

