// SPDX-License-Identifier: Apache-2.0
import { useEffect, useMemo, useState } from "react";
import {
  ArrowLeft,
  Brain,
  Save,
  Check,
  FolderOpen,
  Sparkles,
} from "lucide-react";
import { invoke } from "../../lib/tauri";
import { useChatStore } from "../../stores/chat";

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

          <LearningLogSection />

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

function LearningLogSection() {
  return (
    <section>
      <h2 className="text-xs font-semibold text-gray-400 uppercase tracking-wider mb-3">
        学习日志
      </h2>
      <div className="rounded-lg border border-dashed border-border bg-surface-1 px-6 py-10 text-center">
        <Sparkles size={20} className="text-gray-600 mx-auto mb-3" />
        <p className="text-sm text-gray-400 font-medium mb-1">自进化能力开发中</p>
        <p className="text-xs text-gray-500 max-w-md mx-auto leading-relaxed">
          未来这里会显示 AI 从你的行为中归纳出的新事实，
          每条都需要你确认后才会更新到上方的偏好与记忆中。
        </p>
      </div>
    </section>
  );
}
