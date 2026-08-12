// SPDX-License-Identifier: Apache-2.0
import { FolderOpen, Folder, Clock, ArrowRight, RotateCcw } from "lucide-react";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { useChatStore } from "../stores/chat";
import { WelcomeUsageCard } from "./WelcomeUsageCard";
import { folderName } from "../lib/projects";

/**
 * In-line render of the app's "Crystallization" gem mark — same geometry
 * as the OS icon (rotated rhombus with two facets, the upper one bright,
 * the lower one slightly recessed). Kept as SVG rather than importing the
 * PNG so it scales crisply, stays in lockstep with the brand if the icon
 * is ever redesigned, and avoids an asset-bundling round-trip through
 * the Tauri side.
 */
function GemMark({ size = 28 }: { size?: number }) {
  return (
    <svg
      viewBox="0 0 56 56"
      width={size}
      height={size}
      className="drop-shadow-sm"
      aria-hidden
    >
      <g transform="rotate(7 28 28)">
        {/* Upper facet — fully bright */}
        <polygon points="27.2,8 41,28 15,28" fill="#fff" />
        {/* Lower facet — slightly translucent so the facet seam reads */}
        <polygon points="41,28 28.6,48 15,28" fill="#fff" fillOpacity="0.88" />
      </g>
    </svg>
  );
}

interface Props {
  /**
   * Called when the user clicks an example prompt. The parent (ChatPage)
   * decides what to do — typically: fill the input box.
   */
  onUsePrompt?: (text: string) => void;
  /** Opens Settings directly on the first-class usage dashboard. */
  onOpenUsage?: () => void;
  /** Resume an existing conversation. Always an explicit, labelled act — this
   *  screen must never move the user into old history as a side effect. */
  onOpenSession?: (id: string) => void;
  /** Re-scope the current draft to a project directory (null = standalone).
   *  Only meaningful while the conversation is still a draft. */
  onPickProject?: (cwd: string | null) => void;
}

// Curated short prompts that hint at what the agent can do, mostly bias
// toward "do something useful with my current project" rather than generic
// LLM chat openers.
const EXAMPLES: { title: string; prompt: string }[] = [
  {
    title: "了解当前代码库",
    prompt:
      "概览这个项目的用途、主要模块和推荐阅读顺序。",
  },
  {
    title: "找出快速改进",
    prompt:
      "检查最近提交，找出三个可以快速完成的小缺陷、缺失测试或过期 TODO。",
  },
  {
    title: "实现一个功能",
    prompt:
      "我要添加一个功能。先读取仓库里的约束和规格，再直接实现并验证。",
  },
  {
    title: "审查当前改动",
    prompt:
      "像资深工程师一样审查未提交改动，指出风险、歧义和值得重构的部分。",
  },
];

export function WelcomeScreen({ onUsePrompt, onOpenUsage, onOpenSession, onPickProject }: Props) {
  const { sessions, activeSession, draftSession, activeModel } = useChatStore();

  const scopeCwd = draftSession ? draftSession.cwd : activeSession?.cwd ?? null;
  // While drafting, the project tiles re-scope this blank conversation. The
  // "resume" list is kept visually and verbally separate below, because
  // conflating the two is what used to drop users into old history.
  const scopedSessions = scopeCwd
    ? (sessions ?? []).filter((s) => s.cwd === scopeCwd && s.id !== activeSession?.id)
    : [];
  const recentSessions = (sessions ?? [])
    .filter((s) => s.id !== activeSession?.id)
    .slice(0, 4);

  const browseForProject = async () => {
    const dir = await openDialog({ directory: true, title: "选择项目目录" });
    if (dir) onPickProject?.(dir as string);
  };

  return (
    <div className="flex-1 overflow-y-auto">
      <div className="mx-auto max-w-3xl space-y-4 px-4 py-5 sm:px-6 sm:py-6">
        {/* Hero */}
        <section role="region" aria-label="CodeFactory 欢迎" className="flex flex-wrap items-center gap-3">
          <div className="inline-flex h-10 w-10 shrink-0 items-center justify-center rounded-xl bg-gradient-to-br from-orange-500 to-red-600 shadow-sm">
            <GemMark size={23} />
          </div>
          <div className="min-w-0">
            <h1 className="text-heading font-semibold leading-tight text-gray-100">CodeFactory</h1>
            <p className="mt-0.5 text-label text-gray-400">带上代码库，说明目标，直接交付。</p>
          </div>
          {activeSession && (
            <div className="flex min-w-0 flex-1 basis-full items-center gap-2 rounded-lg border border-border bg-surface-1 px-3 py-2 text-caption sm:ml-auto sm:basis-auto sm:max-w-[360px]">
              <FolderOpen size={12} className="shrink-0 text-gray-400" />
              <span className="min-w-0 flex-1 truncate font-mono text-gray-400" title={activeSession.cwd}>{activeSession.cwd}</span>
              <span aria-hidden className="text-gray-500">·</span>
              <span className="max-w-[120px] truncate text-gray-300" title={activeModel}>{activeModel.split("/").pop() || activeModel}</span>
            </div>
          )}
        </section>

        <WelcomeUsageCard
          anonymous={activeSession?.kind === "anonymous"}
          onOpenUsage={onOpenUsage}
        />

        {/* One quiet line, not a question you must answer first. A blank
            conversation is always valid; attaching a folder is an option the
            user reaches for when they happen to need one. */}
        {draftSession && onPickProject && (
          <div className="flex flex-wrap items-center gap-2 px-1 text-caption text-gray-500">
            <span>{scopeCwd ? "这次在" : "没有指定目录，不会碰任何代码。"}</span>
            {scopeCwd && (
              <span className="inline-flex items-center gap-1 text-gray-300">
                <Folder size={11} className="text-accent" />
                <span className="max-w-[220px] truncate" title={scopeCwd}>{folderName(scopeCwd)}</span>
                <span className="text-gray-500">里干活</span>
              </span>
            )}
            <button
              onClick={() => void browseForProject()}
              className="inline-flex items-center gap-1 rounded text-accent transition-colors hover:underline"
            >
              <FolderOpen size={11} />
              {scopeCwd ? "换一个目录" : "要在某个目录里干活？"}
            </button>
            {scopeCwd && (
              <button
                onClick={() => onPickProject(null)}
                className="rounded text-gray-500 transition-colors hover:text-gray-300 hover:underline"
              >
                取消
              </button>
            )}
          </div>
        )}

        {/* Example prompts */}
        <section className="space-y-2" aria-labelledby="welcome-suggestions-title">
          <h2 id="welcome-suggestions-title" className="px-1 text-body font-semibold text-gray-400">可以试试</h2>
          <div className="grid grid-cols-1 gap-2 min-[520px]:grid-cols-2">
            {EXAMPLES.map((ex) => (
              <button
                key={ex.title}
                onClick={() => onUsePrompt?.(ex.prompt)}
                className="group rounded-lg border border-border bg-surface-1 px-3 py-2.5 text-left transition-colors hover:border-accent/40 hover:bg-surface-2 focus:outline-none focus:ring-2 focus:ring-accent/50"
              >
                <div className="flex items-center justify-between gap-2">
                  <div className="text-body font-medium text-gray-100">{ex.title}</div>
                  <ArrowRight
                    size={12}
                    className="text-gray-600 group-hover:text-accent transition-colors shrink-0"
                  />
                </div>
                <div className="mt-1 line-clamp-2 text-caption leading-relaxed text-gray-400">
                  {ex.prompt}
                </div>
              </button>
            ))}
          </div>
        </section>

        {/* Resume — deliberately the only path back into old history, and it
            says so. Everything above starts something new. */}
        {onOpenSession && (scopedSessions.length > 0 || recentSessions.length > 0) && (
          <div className="space-y-2">
            <div className="text-body text-gray-600 font-semibold px-1 flex items-center gap-1.5">
              <Clock size={14} />
              继续之前的会话
            </div>
            {scopedSessions.length > 0 && (
              <button
                onClick={() => onOpenSession(scopedSessions[0].id)}
                className="w-full rounded border border-border bg-surface-1 px-3 py-2 text-left transition-colors hover:bg-surface-2"
              >
                <div className="flex items-center gap-2">
                  <RotateCcw size={11} className="shrink-0 text-accent" />
                  <span className="min-w-0 flex-1 truncate text-label text-gray-200">
                    接着上次说：{scopedSessions[0].title || "未命名会话"}
                  </span>
                  <span className="shrink-0 text-caption text-gray-600">
                    {folderName(scopedSessions[0].cwd)}
                  </span>
                </div>
              </button>
            )}
            <div className="space-y-1">
              {recentSessions.map((s) => (
                <button
                  key={s.id}
                  onClick={() => onOpenSession(s.id)}
                  className="w-full text-left rounded border border-border bg-surface-1 hover:bg-surface-2 px-3 py-1.5 transition-colors"
                >
                  <div className="flex items-center gap-2">
                    <span className="text-label text-gray-300 truncate flex-1">{s.title || "未命名"}</span>
                    <span className="max-w-[140px] truncate font-mono text-caption text-gray-600" title={s.cwd}>
                      {s.kind === "quick" ? "独立任务" : folderName(s.cwd)}
                    </span>
                  </div>
                </button>
              ))}
            </div>
          </div>
        )}
      </div>
    </div>
  );
}
