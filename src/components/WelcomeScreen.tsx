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
    title: "Explain this codebase",
    prompt:
      "Give me a high-level tour of this project: what does it do, what are the main modules, and where would I start reading?",
  },
  {
    title: "Find quick wins",
    prompt:
      "Skim recent commits and identify three quick wins — small bugs, missing tests, or stale TODOs that would take less than 30 minutes each.",
  },
  {
    title: "Add a feature",
    prompt:
      "I want to add [your feature here]. Draft a short spec, then break it into 3-5 atomic tasks I can review before you implement.",
  },
  {
    title: "Review my changes",
    prompt:
      "Look at my uncommitted changes and review them like a senior engineer — call out anything risky, unclear, or worth refactoring.",
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
      <div className="max-w-2xl mx-auto px-6 py-8 space-y-6">
        {/* Hero */}
        <div className="text-center space-y-2">
          <div className="inline-flex items-center justify-center w-14 h-14 rounded-2xl bg-gradient-to-br from-orange-500 to-red-600 shadow-lg">
            <GemMark size={32} />
          </div>
          <h1 className="text-2xl font-semibold text-gray-100">CodeFactory</h1>
          <p className="text-sm text-gray-500">
            AI coding assistant. Bring a folder, set a goal, ship faster.
          </p>
        </div>

        {/* Current context */}
        {activeSession && (
          <div className="rounded-lg border border-border bg-surface-1 px-3 py-2.5 flex items-center gap-3 text-xs">
            <FolderOpen size={13} className="text-gray-500 shrink-0" />
            <span className="text-gray-400 font-mono truncate flex-1" title={activeSession.cwd}>
              {activeSession.cwd}
            </span>
            <span className="text-gray-600">·</span>
            <span className="text-gray-400 truncate max-w-[180px]" title={activeModel}>
              {activeModel.split("/").pop() || activeModel}
            </span>
          </div>
        )}

        <WelcomeUsageCard
          anonymous={activeSession?.kind === "anonymous"}
          onOpenUsage={onOpenUsage}
        />

        {/* Example prompts */}
        <div className="space-y-2">
          <div className="text-[11px] uppercase tracking-wider text-gray-600 font-semibold px-1">
            Try asking
          </div>
          <div className="grid grid-cols-2 gap-2">
            {EXAMPLES.map((ex) => (
              <button
                key={ex.title}
                onClick={() => onUsePrompt?.(ex.prompt)}
                className="group text-left rounded-lg border border-border bg-surface-1 hover:bg-surface-2 hover:border-accent/40 px-3 py-2.5 transition-colors"
              >
                <div className="flex items-center justify-between gap-2">
                  <div className="text-xs font-medium text-gray-200">{ex.title}</div>
                  <ArrowRight
                    size={12}
                    className="text-gray-600 group-hover:text-accent transition-colors shrink-0"
                  />
                </div>
                <div className="mt-1 text-[11px] text-gray-500 line-clamp-2 leading-relaxed">
                  {ex.prompt}
                </div>
              </button>
            ))}
          </div>
        </div>

        {/* Recent sessions */}
        {recentSessions.length > 0 && (
          <div className="space-y-2">
            <div className="text-[11px] uppercase tracking-wider text-gray-600 font-semibold px-1 flex items-center gap-1.5">
              <Clock size={11} />
              Recent sessions
            </div>
            <div className="space-y-1">
              {recentSessions.map((s) => (
                <button
                  key={s.id}
                  onClick={() => selectSession(s.id)}
                  className="w-full text-left rounded border border-border bg-surface-1 hover:bg-surface-2 px-3 py-1.5 transition-colors"
                >
                  <div className="flex items-center gap-2">
                    <span className="text-xs text-gray-300 truncate flex-1">{s.title || "Untitled"}</span>
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
