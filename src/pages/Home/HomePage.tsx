// SPDX-License-Identifier: Apache-2.0
import { useEffect, useState } from "react";
import {
  Plus,
  Zap,
  User,
  Settings as SettingsIcon,
  Puzzle,
  Moon,
  Sun,
  Monitor,
  FolderOpen,
  Clock,
} from "lucide-react";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { useChatStore } from "../../stores/chat";
import { useSettingsStore } from "../../stores/settings";
import {
  createQuickSession,
  listQuickSessions,
} from "../../lib/tauri";
import { formatRelativeTime } from "../../lib/time";
import type { Session, Theme } from "../../lib/tauri";

interface HomePageProps {
  onOpenProject: (sessionId: string) => void;
  onOpenSkills: () => void;
  onOpenSettings: () => void;
  onOpenProfile: () => void;
}

/**
 * Home — landing screen and primary entry point.
 *
 * Three entry tiers reflecting use intensity:
 *   • New project    — full software-factory flow (heavy)
 *   • Quick task     — one-off assistant task (light, no project residue)
 *   • Recent project — resume in workspace (medium)
 */
function Brand() {
  return (
    <div className="flex items-center gap-3">
      <svg viewBox="0 0 56 56" width={32} height={32} className="drop-shadow-sm" aria-hidden>
        <g transform="rotate(7 28 28)">
          <polygon points="27.2,8 41,28 15,28" fill="currentColor" />
          <polygon points="41,28 28.6,48 15,28" fill="currentColor" fillOpacity="0.7" />
        </g>
      </svg>
      <div>
        <h1 className="text-lg font-semibold text-gray-200 leading-tight">CodeFactory</h1>
        <p className="text-xs text-gray-500">软件工厂 · 本地助手 · 自进化</p>
      </div>
    </div>
  );
}

export function HomePage({
  onOpenProject,
  onOpenSkills,
  onOpenSettings,
  onOpenProfile,
}: HomePageProps) {
  const { sessions, loadSessions, createSession, activeModel } = useChatStore();
  const { settings, setTheme } = useSettingsStore();
  const [quickSessions, setQuickSessions] = useState<Session[]>([]);

  const refreshQuickSessions = () => {
    listQuickSessions()
      .then(setQuickSessions)
      .catch(() => {
        /* first run / no quick history yet — leave empty; the card still works */
      });
  };

  useEffect(() => {
    loadSessions();
    refreshQuickSessions();
  }, []);

  const handleNewProject = async () => {
    const dir = await openDialog({ directory: true, title: "选择项目目录" });
    if (!dir) return;
    const session = await createSession(dir as string, activeModel);
    if (session) onOpenProject(session.id);
  };

  const handleQuickTask = async () => {
    // Always start a FRESH quick task. The primary "快速任务" card is an action,
    // so it must not drop the user back into their last quick session — that
    // was a continue-latest bug. Resuming a specific quick task is what the
    // "最近快速任务" list below is for; entering an existing session should only
    // ever happen by clicking it in a list.
    try {
      const session = await createQuickSession(activeModel);
      onOpenProject(session.id);
    } catch (e) {
      // eslint-disable-next-line no-alert
      alert(`快速任务启动失败：${String(e)}`);
    }
  };

  const handleNewQuickTask = async () => {
    // Always-fresh parallel quick task with its own scratch dir.
    try {
      const session = await createQuickSession(activeModel);
      onOpenProject(session.id);
    } catch (e) {
      // eslint-disable-next-line no-alert
      alert(`新建快速任务失败：${String(e)}`);
    }
  };

  const handleProfile = () => {
    onOpenProfile();
  };

  const recent = [...sessions]
    .sort((a, b) => b.updated_at - a.updated_at)
    .slice(0, 6);

  return (
    <div className="h-full flex flex-col bg-surface-0">

      {/* ── Top bar ───────────────────────────────────────────────────────── */}
      <header className="flex items-center justify-between px-6 py-4 border-b border-border bg-surface-1">
        <Brand />
        <div className="flex items-center gap-2">
          {/* Theme toggle */}
          <div className="flex items-center rounded border border-border overflow-hidden">
            {([
              { v: "dark",   Icon: Moon,    label: "深色" },
              { v: "light",  Icon: Sun,     label: "浅色" },
              { v: "system", Icon: Monitor, label: "跟随系统" },
            ] as { v: Theme; Icon: React.ElementType; label: string }[]).map(({ v, Icon, label }) => (
              <button
                key={v}
                onClick={() => setTheme(v)}
                title={label}
                className={`p-1.5 transition-colors ${
                  settings?.theme === v
                    ? "bg-surface-3 text-accent"
                    : "text-gray-600 hover:text-gray-300 hover:bg-surface-3"
                }`}
              >
                <Icon size={13} />
              </button>
            ))}
          </div>
          <button
            onClick={onOpenSkills}
            className="p-2 rounded text-gray-600 hover:text-gray-300 hover:bg-surface-3 transition-colors"
            title="技能库"
          >
            <Puzzle size={14} />
          </button>
          <button
            onClick={onOpenSettings}
            className="p-2 rounded text-gray-600 hover:text-gray-300 hover:bg-surface-3 transition-colors"
            title="设置"
          >
            <SettingsIcon size={14} />
          </button>
        </div>
      </header>

      {/* ── Body ──────────────────────────────────────────────────────────── */}
      <div className="flex-1 overflow-y-auto">
        <div className="max-w-4xl mx-auto px-6 py-10 space-y-10">

          {/* ── Three primary entries ─────────────────────────────────── */}
          <section>
            <h2 className="text-xs font-semibold uppercase tracking-wider text-gray-400 mb-3">开始</h2>
            <div className="grid grid-cols-3 gap-3">
              <EntryCard
                Icon={Plus}
                title="新建项目"
                desc="从需求开始，AI 拆解任务并交付完整软件"
                tone="primary"
                onClick={handleNewProject}
              />
              <EntryCard
                Icon={Zap}
                title="快速任务"
                desc="不开项目，处理一个零碎的助手任务"
                tone="muted"
                onClick={handleQuickTask}
              />
              <EntryCard
                Icon={User}
                title="我的画像"
                desc="AI 对你的理解，可查看可编辑"
                tone="muted"
                onClick={handleProfile}
              />
            </div>
          </section>

          {/* ── Recent quick tasks (multi-session switcher) ───────────── */}
          {quickSessions.length > 0 && (
            <section>
              <h2 className="text-xs font-semibold uppercase tracking-wider text-gray-400 mb-3">
                最近快速任务
              </h2>
              <div className="grid grid-cols-2 gap-3">
                <button
                  onClick={handleNewQuickTask}
                  className="group flex items-center gap-2 p-4 rounded-lg border border-dashed border-border bg-surface-1 hover:bg-surface-2 hover:border-gray-500 transition-colors"
                >
                  <Plus size={14} className="text-gray-500 group-hover:text-accent" />
                  <span className="text-sm text-gray-400 group-hover:text-gray-200">
                    新建快速任务
                  </span>
                </button>
                {quickSessions.map((s) => (
                  <button
                    key={s.id}
                    onClick={() => onOpenProject(s.id)}
                    className="group text-left p-4 rounded-lg border border-border bg-surface-1 hover:bg-surface-2 hover:border-gray-500 transition-colors"
                  >
                    <div className="flex items-start justify-between gap-2 mb-2">
                      <h3 className="text-sm font-medium text-gray-200 truncate flex-1">
                        {s.title || "快速任务"}
                      </h3>
                      <Zap size={11} className="mt-1 text-gray-600 group-hover:text-accent" />
                    </div>
                    <p className="text-[10px] text-gray-600">
                      {formatRelativeTime(s.updated_at)}
                    </p>
                  </button>
                ))}
              </div>
            </section>
          )}

          {/* ── Recent projects ───────────────────────────────────────── */}
          <section>
            <h2 className="text-xs font-semibold uppercase tracking-wider text-gray-400 mb-3">
              最近项目
            </h2>
            {recent.length === 0 ? (
              <div className="p-8 rounded-lg border border-border bg-surface-1 text-center">
                <FolderOpen size={28} className="mx-auto mb-2 text-gray-600" />
                <p className="text-sm text-gray-500">
                  还没有项目。点上面的「新建项目」开始第一个。
                </p>
              </div>
            ) : (
              <div className="grid grid-cols-2 gap-3">
                {recent.map((s) => (
                  <button
                    key={s.id}
                    onClick={() => onOpenProject(s.id)}
                    className="group text-left p-4 rounded-lg border border-border bg-surface-1 hover:bg-surface-2 hover:border-gray-500 transition-colors"
                  >
                    <div className="flex items-start justify-between gap-2 mb-2">
                      <h3 className="text-sm font-medium text-gray-200 truncate flex-1">
                        {s.title || "未命名项目"}
                      </h3>
                      <Clock size={11} className="mt-1 text-gray-600 group-hover:text-gray-400" />
                    </div>
                    <p className="text-[11px] text-gray-500 font-mono truncate mb-1">{s.cwd}</p>
                    <p className="text-[10px] text-gray-600">
                      {formatRelativeTime(s.updated_at)}
                    </p>
                  </button>
                ))}
              </div>
            )}
          </section>

        </div>
      </div>
    </div>
  );
}

// ── EntryCard ────────────────────────────────────────────────────────────────

interface EntryCardProps {
  Icon: React.ElementType;
  title: string;
  desc: string;
  tone: "primary" | "muted";
  onClick: () => void;
}

function EntryCard({ Icon, title, desc, tone, onClick }: EntryCardProps) {
  const primary = tone === "primary";
  return (
    <button
      onClick={onClick}
      className={`text-left p-5 rounded-lg border transition-colors ${
        primary
          ? "border-accent bg-accent/10 hover:bg-accent/20 text-gray-200"
          : "border-border bg-surface-1 hover:bg-surface-2 hover:border-gray-500"
      }`}
    >
      <Icon
        size={18}
        className={primary ? "text-accent mb-3" : "text-gray-400 mb-3"}
      />
      <h3 className={`text-sm font-medium mb-1 ${primary ? "text-gray-200" : "text-gray-300"}`}>
        {title}
      </h3>
      <p className="text-xs text-gray-500 leading-relaxed">{desc}</p>
    </button>
  );
}
