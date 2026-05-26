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

interface LearningEvent {
  id: string;
  session_id: string;
  cwd: string;
  observation: string;
  suggestion: string;
  status: "pending" | "accepted" | "rejected";
  created_at: string;
  decided_at: string | null;
}

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
 *   • 个人偏好         — static placeholders (until self-evolution lands)
 *   • 项目记忆 (.md)   — read / edit one project's memory.md at a time
 *   • 学习日志         — placeholder for self-evolution events
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

          <PreferencesSection />

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
// PreferencesSection — static placeholders until self-evolution wires this up
// ─────────────────────────────────────────────────────────────────────────────

function PreferencesSection() {
  return (
    <section>
      <h2 className="text-xs font-semibold text-gray-400 uppercase tracking-wider mb-3">
        个人偏好
      </h2>
      <div className="rounded-lg border border-border bg-surface-1 divide-y divide-border">
        <PrefRow label="自主程度" value="中等" hint="重要操作需要确认，常规操作自主执行" />
        <PrefRow label="沟通风格" value="简洁" hint="少废话、直接说结果，需要时再展开" />
        <PrefRow label="测试习惯" value="TDD" hint="写新功能时先写测试" />
        <PrefRow label="代码风格" value="（自动学习中）" hint="AI 会从你接受/拒绝的修改中归纳" />
      </div>
      <p className="mt-2 text-[11px] text-gray-500">
        这些偏好将由「自进化」能力自动维护并接受你的修改。当前为占位值。
      </p>
    </section>
  );
}

function PrefRow({ label, value, hint }: { label: string; value: string; hint: string }) {
  return (
    <div className="flex items-start gap-4 px-4 py-3">
      <div className="w-24 shrink-0">
        <div className="text-xs font-medium text-gray-300">{label}</div>
      </div>
      <div className="flex-1">
        <div className="text-sm text-gray-200">{value}</div>
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
// LearningLogSection — placeholder until self-evolution lands
// ─────────────────────────────────────────────────────────────────────────────

function LearningLogSection({ selectedCwd }: { selectedCwd: string | null }) {
  const [events, setEvents] = useState<LearningEvent[]>([]);
  const [loading, setLoading] = useState(false);
  const [busyId, setBusyId] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  const reload = async (cwd: string) => {
    setLoading(true);
    setError(null);
    try {
      const list = await invoke<LearningEvent[]>("list_learning_events", { cwd });
      setEvents(list);
    } catch (e) {
      setError(String(e));
    } finally {
      setLoading(false);
    }
  };

  useEffect(() => {
    if (selectedCwd) reload(selectedCwd);
  }, [selectedCwd]);

  const handleAccept = async (id: string) => {
    setBusyId(id);
    setError(null);
    try {
      await invoke("accept_learning_event", { eventId: id });
      if (selectedCwd) await reload(selectedCwd);
    } catch (e) {
      setError(String(e));
    } finally {
      setBusyId(null);
    }
  };

  const handleReject = async (id: string) => {
    setBusyId(id);
    setError(null);
    try {
      await invoke("reject_learning_event", { eventId: id });
      if (selectedCwd) await reload(selectedCwd);
    } catch (e) {
      setError(String(e));
    } finally {
      setBusyId(null);
    }
  };

  const pending = events.filter((e) => e.status === "pending");
  const decided = events.filter((e) => e.status !== "pending");

  return (
    <section>
      <h2 className="text-xs font-semibold text-gray-400 uppercase tracking-wider mb-3">
        学习日志
        {pending.length > 0 && (
          <span className="ml-2 text-[10px] font-normal text-accent normal-case">
            {pending.length} 条待审
          </span>
        )}
      </h2>

      {!selectedCwd ? (
        <p className="text-xs text-gray-500 text-center py-6">选一个项目以查看学习记录</p>
      ) : loading ? (
        <p className="text-xs text-gray-500 text-center py-6 flex items-center justify-center gap-2">
          <Loader2 size={12} className="animate-spin" /> 加载中...
        </p>
      ) : events.length === 0 ? (
        <div className="rounded-lg border border-dashed border-border bg-surface-1 px-6 py-10 text-center">
          <Sparkles size={20} className="text-gray-600 mx-auto mb-3" />
          <p className="text-sm text-gray-400 font-medium mb-1">暂无学习记录</p>
          <p className="text-xs text-gray-500 max-w-md mx-auto leading-relaxed">
            每个任务 session 结束后，AI 会自动总结观察到的事实，
            出现在这里等你审核。审核通过的会写入项目记忆并影响未来对话。
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

          {decided.length > 0 && (
            <details className="rounded-lg border border-border bg-surface-1">
              <summary className="px-4 py-2 text-xs text-gray-500 cursor-pointer hover:text-gray-300 select-none">
                历史决定 ({decided.length})
              </summary>
              <ul className="border-t border-border divide-y divide-border">
                {decided.map((e) => (
                  <li key={e.id} className="px-4 py-2 text-[11px]">
                    <span
                      className={
                        e.status === "accepted"
                          ? "text-green-700 dark:text-green-400 mr-2"
                          : "text-gray-500 mr-2"
                      }
                    >
                      {e.status === "accepted" ? "✓ 采纳" : "✕ 拒绝"}
                    </span>
                    <span className="text-gray-400">{e.observation}</span>
                  </li>
                ))}
              </ul>
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
  return (
    <div className="rounded-lg border border-accent/40 bg-accent/5 p-4">
      <div className="flex items-start gap-2 mb-2">
        <Sparkles size={12} className="text-accent mt-0.5 shrink-0" />
        <p className="text-xs text-gray-300 leading-relaxed">{event.observation}</p>
      </div>
      <div className="rounded bg-surface-2 border border-border px-3 py-2 mb-3">
        <div className="text-[10px] uppercase tracking-wider text-gray-500 mb-1">建议写入记忆</div>
        <p className="text-[12px] text-gray-200 font-mono leading-relaxed">{event.suggestion}</p>
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
          采纳并写入记忆
        </button>
      </div>
    </div>
  );
}

