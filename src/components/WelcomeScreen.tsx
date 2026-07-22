// SPDX-License-Identifier: Apache-2.0
import { FolderOpen, Clock, ArrowRight } from "lucide-react";
import { useChatStore } from "../stores/chat";
import { WelcomeUsageCard } from "./WelcomeUsageCard";

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

export function WelcomeScreen({ onUsePrompt, onOpenUsage }: Props) {
  const { sessions, activeSession, activeModel, selectSession } = useChatStore();

  // Show up to 4 most recent sessions other than the currently-active one.
  const recentSessions = sessions
    .filter((s) => s.id !== activeSession?.id)
    .slice(0, 4);

  return (
    <div className="flex-1 overflow-y-auto">
      <div className="mx-auto max-w-3xl space-y-4 px-4 py-5 sm:px-6 sm:py-6">
        {/* Hero */}
        <section role="region" aria-label="CodeFactory 欢迎" className="flex flex-wrap items-center gap-3">
          <div className="inline-flex h-10 w-10 shrink-0 items-center justify-center rounded-xl bg-gradient-to-br from-orange-500 to-red-600 shadow-sm">
            <GemMark size={23} />
          </div>
          <div className="min-w-0">
            <h1 className="text-xl font-semibold leading-tight text-gray-100">CodeFactory</h1>
            <p className="mt-0.5 text-xs text-gray-400">带上代码库，说明目标，直接交付。</p>
          </div>
          {activeSession && (
            <div className="flex min-w-0 flex-1 basis-full items-center gap-2 rounded-lg border border-border bg-surface-1 px-3 py-2 text-[11px] sm:ml-auto sm:basis-auto sm:max-w-[360px]">
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

        {/* Example prompts */}
        <section className="space-y-2" aria-labelledby="welcome-suggestions-title">
          <h2 id="welcome-suggestions-title" className="px-1 text-[11px] font-semibold tracking-wide text-gray-400">可以试试</h2>
          <div className="grid grid-cols-1 gap-2 min-[520px]:grid-cols-2">
            {EXAMPLES.map((ex) => (
              <button
                key={ex.title}
                onClick={() => onUsePrompt?.(ex.prompt)}
                className="group rounded-lg border border-border bg-surface-1 px-3 py-2.5 text-left transition-colors hover:border-accent/40 hover:bg-surface-2 focus:outline-none focus:ring-2 focus:ring-accent/50"
              >
                <div className="flex items-center justify-between gap-2">
                  <div className="text-xs font-medium text-gray-100">{ex.title}</div>
                  <ArrowRight
                    size={12}
                    className="text-gray-600 group-hover:text-accent transition-colors shrink-0"
                  />
                </div>
                <div className="mt-1 line-clamp-2 text-[11px] leading-relaxed text-gray-400">
                  {ex.prompt}
                </div>
              </button>
            ))}
          </div>
        </section>

        {/* Recent sessions */}
        {recentSessions.length > 0 && (
          <div className="space-y-2">
            <div className="text-[11px] uppercase tracking-wider text-gray-600 font-semibold px-1 flex items-center gap-1.5">
              <Clock size={11} />
              最近会话
            </div>
            <div className="space-y-1">
              {recentSessions.map((s) => (
                <button
                  key={s.id}
                  onClick={() => selectSession(s.id)}
                  className="w-full text-left rounded border border-border bg-surface-1 hover:bg-surface-2 px-3 py-1.5 transition-colors"
                >
                  <div className="flex items-center gap-2">
                    <span className="text-xs text-gray-300 truncate flex-1">{s.title || "未命名"}</span>
                    <span className="text-[10px] text-gray-600 font-mono truncate max-w-[140px]" title={s.cwd}>
                      {s.cwd.split(/[\\/]/).pop()}
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
