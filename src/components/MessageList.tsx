// SPDX-License-Identifier: Apache-2.0
import { useEffect, useState } from "react";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import type { Components } from "react-markdown";
import { createHighlighter, type Highlighter } from "shiki";
import { Check, Copy, ChevronDown } from "lucide-react";
import { ToolCallCard } from "./ToolCallCard";
import { WelcomeScreen } from "./WelcomeScreen";
import { useStickyAutoScroll } from "./useStickyAutoScroll";
import { RememberButton } from "./RememberButton";
import { formatDuration, useNowTick } from "../lib/duration";
import type { UIMessage } from "../stores/chat";
import type { TurnSegment } from "../stores/chatEvents";

interface Props {
  messages: UIMessage[];
  streaming: boolean;
  /** Working directory of the active session — used to scope the
   *  "Remember" button's writes to the right project-memory file. */
  cwd?: string | null;
  /** Called when the user picks an example prompt from the welcome screen. */
  onUsePrompt?: (text: string) => void;
  onOpenUsage?: () => void;
}

// ── Shiki singleton ──────────────────────────────────────────────────────────
let _hlPromise: Promise<Highlighter> | null = null;
function getHighlighter(): Promise<Highlighter> {
  if (!_hlPromise) {
    _hlPromise = createHighlighter({
      themes: ["github-dark"],
      langs: [
        "typescript", "javascript", "tsx", "jsx",
        "rust", "python", "bash", "sh", "json",
        "yaml", "toml", "html", "css", "sql",
        "markdown", "go", "java", "cpp", "c",
        "diff",
      ],
    });
  }
  return _hlPromise;
}

// ── Code block with language label + copy button ─────────────────────────────
function CodeBlock({ lang, code }: { lang: string; code: string }) {
  const [html, setHtml] = useState<string | null>(null);
  const [copied, setCopied] = useState(false);

  useEffect(() => {
    let cancelled = false;
    getHighlighter().then((hl) => {
      if (cancelled) return;
      try {
        const resolved = hl.getLoadedLanguages().includes(lang as never) ? lang : "text";
        setHtml(hl.codeToHtml(code, { lang: resolved, theme: "github-dark" }));
      } catch {
        setHtml(null);
      }
    });
    return () => { cancelled = true; };
  }, [lang, code]);

  const handleCopy = () => {
    navigator.clipboard.writeText(code).then(() => {
      setCopied(true);
      setTimeout(() => setCopied(false), 1500);
    });
  };

  return (
    <div className="my-2 rounded-md overflow-hidden border border-border bg-[#0d1117]">
      <div className="flex items-center justify-between px-3 py-1 bg-surface-3 border-b border-border">
        <span className="text-[10px] uppercase tracking-wide text-gray-500 font-sans">
          {lang === "text" ? "code" : lang}
        </span>
        <button
          onClick={handleCopy}
          className="flex items-center gap-1 text-[10px] text-gray-500 hover:text-gray-200 transition-colors font-sans"
          title="Copy"
        >
          {copied ? <><Check size={11} className="text-green-400" /> Copied</> : <><Copy size={11} /> Copy</>}
        </button>
      </div>
      {html ? (
        <div
          className="text-xs overflow-x-auto [&>pre]:!p-3 [&>pre]:!bg-transparent"
          dangerouslySetInnerHTML={{ __html: html }}
        />
      ) : (
        <pre className="p-3 overflow-x-auto text-xs leading-relaxed">
          <code>{code}</code>
        </pre>
      )}
    </div>
  );
}

// ── Rich markdown component overrides ────────────────────────────────────────
const markdownComponents: Components = {
  code({ className, children, ...props }) {
    const match = /language-(\w+)/.exec(className || "");
    const isBlock = !!match;
    const code = String(children).replace(/\n$/, "");
    if (isBlock) {
      return <CodeBlock lang={match![1]} code={code} />;
    }
    return (
      <code className="bg-surface-3 px-1 py-0.5 rounded text-[12px] font-mono text-amber-700 dark:text-amber-200" {...props}>
        {children}
      </code>
    );
  },
  h1: ({ children }) => (
    <h1 className="mt-6 mb-3 pb-2 border-b border-border text-lg font-semibold text-gray-100">{children}</h1>
  ),
  h2: ({ children }) => (
    <h2 className="mt-5 mb-2 text-base font-semibold text-gray-100">{children}</h2>
  ),
  h3: ({ children }) => (
    <h3 className="mt-4 mb-2 text-sm font-semibold text-gray-200">{children}</h3>
  ),
  h4: ({ children }) => (
    <h4 className="mt-3 mb-1.5 text-sm font-medium text-gray-300">{children}</h4>
  ),
  p: ({ children }) => <p className="my-2 leading-relaxed">{children}</p>,
  ul: ({ children }) => (
    <ul className="my-2 ml-4 space-y-1 list-disc marker:text-gray-600">{children}</ul>
  ),
  ol: ({ children }) => (
    <ol className="my-2 ml-4 space-y-1 list-decimal marker:text-gray-500">{children}</ol>
  ),
  li: ({ children }) => <li className="leading-relaxed pl-1">{children}</li>,
  blockquote: ({ children }) => (
    <blockquote className="my-3 pl-3 border-l-2 border-accent/60 bg-accent/5 py-1 text-gray-300 italic">
      {children}
    </blockquote>
  ),
  hr: () => <hr className="my-4 border-border/60" />,
  table: ({ children }) => (
    <div className="my-3 overflow-x-auto">
      <table className="w-full border-collapse text-xs">{children}</table>
    </div>
  ),
  thead: ({ children }) => <thead className="bg-surface-3">{children}</thead>,
  th: ({ children }) => (
    <th className="border border-border px-2 py-1 text-left font-semibold text-gray-200">{children}</th>
  ),
  td: ({ children }) => (
    <td className="border border-border px-2 py-1 text-gray-300">{children}</td>
  ),
  a: ({ children, href }) => (
    <a href={href} target="_blank" rel="noreferrer" className="text-accent hover:underline">
      {children}
    </a>
  ),
  strong: ({ children }) => <strong className="font-semibold text-gray-100">{children}</strong>,
  em: ({ children }) => <em className="italic text-gray-200">{children}</em>,
};

// ── Typing dots — replaces the block cursor ──────────────────────────────────
function TypingDots() {
  return (
    <span className="inline-flex items-center gap-1 ml-1 align-middle">
      <span className="w-1 h-1 rounded-full bg-accent animate-typing-dot" style={{ animationDelay: "0ms" }} />
      <span className="w-1 h-1 rounded-full bg-accent animate-typing-dot" style={{ animationDelay: "150ms" }} />
      <span className="w-1 h-1 rounded-full bg-accent animate-typing-dot" style={{ animationDelay: "300ms" }} />
    </span>
  );
}

// ── Main component ───────────────────────────────────────────────────────────
export function MessageList({ messages, streaming, cwd, onUsePrompt, onOpenUsage }: Props) {
  // Use the first message's id as the conversation identity. Different
  // sessions have different first messages → the scroll hook treats it as a
  // session change and re-pins to the bottom. Empty list → null (fine; no
  // scroller rendered anyway).
  const conversationKey = messages[0]?.id ?? null;
  const contentSignal = messages.length === 0
    ? null
    : `${messages.length}:${messages[messages.length - 1]?.id ?? ""}`;
  const { scrollerRef, pinned, hasNewContent, jumpToBottom } = useStickyAutoScroll(
    conversationKey,
    contentSignal,
  );

  if (messages.length === 0) {
    return <WelcomeScreen onUsePrompt={onUsePrompt} onOpenUsage={onOpenUsage} />;
  }

  const visible = messages.filter((m) => m.role !== "tool" && m.role !== "system");
  const lastAssistantId =
    [...visible].reverse().find((m) => m.role === "assistant")?.id ?? null;

  return (
    <div className="relative flex-1 min-h-0">
      <div
        ref={scrollerRef}
        className="absolute inset-0 overflow-y-auto px-4 py-4 space-y-5"
      >
        {visible.map((msg) => (
          <MessageRow
            key={msg.id}
            msg={msg}
            isStreamingTail={streaming && msg.id === lastAssistantId}
            cwd={cwd ?? null}
          />
        ))}
      </div>

      {!pinned && (
        <button
          onClick={jumpToBottom}
          className={
            hasNewContent
              ? "absolute bottom-3 left-1/2 -translate-x-1/2 z-10 flex items-center gap-1.5 px-3 py-1.5 rounded-full border border-accent/60 bg-accent/15 text-accent text-[11px] font-medium shadow-lg hover:bg-accent/25 transition-colors animate-pulse"
              : "absolute bottom-3 left-1/2 -translate-x-1/2 z-10 flex items-center gap-1 px-2.5 py-1 rounded-full border border-border bg-surface-2 text-[11px] text-gray-300 shadow-lg hover:bg-surface-3 transition-colors"
          }
          title={
            hasNewContent
              ? "New content arrived — click to jump to the latest and resume auto-scroll"
              : "Jump to the latest message and resume auto-scroll"
          }
        >
          <ChevronDown size={12} />
          {hasNewContent ? "New content" : "Jump to latest"}
        </button>
      )}
    </div>
  );
}

function SuccessfulToolGroup({ tools }: { tools: NonNullable<UIMessage["toolCalls"]> }) {
  const [open, setOpen] = useState(false);
  return (
    <div className="my-0.5 border-b border-border/60">
      <button
        type="button"
        aria-label={`${open ? "收起" : "查看"} ${tools.length} 个已完成操作`}
        onClick={() => setOpen((value) => !value)}
        className="flex min-h-7 w-full items-center gap-1.5 px-2 text-left text-[11px] text-gray-600 transition-colors hover:bg-surface-3 hover:text-gray-400"
      >
        <Check size={11} className="text-green-600/70" />
        <span>已完成 {tools.length} 个操作</span>
        <ChevronDown size={11} className={`ml-auto transition-transform ${open ? "rotate-180" : ""}`} />
      </button>
      {open && <div className="pl-3">{tools.map((tool) => <ToolCallCard key={tool.id} tc={tool} />)}</div>}
    </div>
  );
}

function isQuietSuccess(tool: NonNullable<UIMessage["toolCalls"]>[number] | undefined): boolean {
  return Boolean(tool && tool.status === "done" && !tool.isError);
}

function MessageRow({ msg, isStreamingTail, cwd }: { msg: UIMessage; isStreamingTail: boolean; cwd: string | null }) {
  const isUser = msg.role === "user";
  // Must run unconditionally (before the early return) to satisfy the rules
  // of hooks. Only the live streaming tail arms the 1s ticker; for every
  // other row `active` is false, so this is inert.
  const nowMs = useNowTick(isStreamingTail);
  const [showAllSteps, setShowAllSteps] = useState(false);

  // A persisted turn failure (provider error that killed the turn). Red
  // notice with the raw error so it survives reloads — the 2026-07-21
  // interruptions left zero trace because errors were transient events.
  if (msg.completionState === "turn_error") {
    return (
      <div className="flex justify-center">
        <div className="max-w-[85%] rounded border border-red-500/30 bg-red-500/10 px-2.5 py-1 text-[11px] leading-snug text-red-800 dark:text-red-200 break-words">
          回合中断:{msg.content.replace(/^回合中断[::]\s*/, "")}
        </div>
      </div>
    );
  }

  // Verification-incomplete warning: the reply stands, amber notice below.
  if (msg.completionState === "gate_warning") {
    return (
      <div className="flex justify-center">
        <div className="max-w-[85%] rounded border border-amber-500/30 bg-amber-500/10 px-2.5 py-1 text-[11px] leading-snug text-amber-800 dark:text-amber-200 break-words whitespace-pre-wrap">
          {msg.content}
        </div>
      </div>
    );
  }

  // Neutral runtime notices (e.g. images stripped for a no-vision model).
  if (msg.completionState === "turn_notice") {
    return (
      <div className="flex justify-center">
        <div className="max-w-[85%] rounded border border-sky-500/30 bg-sky-500/10 px-2.5 py-1 text-[11px] leading-snug text-sky-800 dark:text-sky-200 break-words">
          {msg.content}
        </div>
      </div>
    );
  }

  // Completion review is an internal control loop. Persisted recovery
  // instructions and rejected drafts remain available to the agent's history,
  // but never become chat content: they are neither user input nor a useful
  // assistant answer.
  if (
    msg.completionState === "gate_recovery" ||
    msg.completionState === "gate_ready" ||
    msg.completionState === "rejected_candidate"
  ) {
    return null;
  }

  if (isUser) {
    return (
      <div className="flex justify-end">
        <div className="max-w-[85%] bg-surface-3 rounded-2xl rounded-br-sm px-4 py-2.5">
          <p className="text-sm text-gray-200 whitespace-pre-wrap">{msg.content}</p>
        </div>
      </div>
    );
  }

  const showThinkingHint =
    isStreamingTail &&
    !msg.content &&
    !msg.reviewProgress &&
    (!msg.toolCalls || msg.toolCalls.length === 0) &&
    (!msg.transportRetries || msg.transportRetries.length === 0);
  // Show Remember only once streaming has settled — a half-written
  // message isn't worth saving as a fact.
  const showRemember = !!cwd && !isStreamingTail && !!msg.content;

  // Per-turn duration: ticks live (off `createdAt`) while this is the
  // streaming tail, then shows the frozen total once the turn settled.
  const durationLabel = isStreamingTail
    ? formatDuration(Math.max(0, nowMs - msg.createdAt))
    : msg.durationMs != null
      ? formatDuration(msg.durationMs)
      : null;

  // Turn timeline: when segments exist (live-streamed turns), render
  // narration and tool cards in ARRIVAL order — mid-turn narration as light
  // step lines, only the final segment as full prose. Without segments
  // (hydrated history), fall back to the classic cards-then-content layout;
  // persisted turns are already split into separate interleaved rows.
  const timeline = msg.segments && msg.segments.length > 0 ? msg.segments : null;
  const toolById = new Map((msg.toolCalls ?? []).map((tc) => [tc.id, tc]));
  const lastTextIndex = timeline
    ? timeline.reduce((acc, s, i) => (s.kind === "text" ? i : acc), -1)
    : -1;
  // Long turns: collapse everything before the visible tail window.
  const COLLAPSE_THRESHOLD = 10;
  const TAIL_VISIBLE = 4;
  const collapsible = timeline && timeline.length > COLLAPSE_THRESHOLD;
  const visibleFrom = collapsible && !showAllSteps ? timeline.length - TAIL_VISIBLE : 0;
  const hiddenSteps = collapsible && !showAllSteps ? visibleFrom : 0;
  const groupedTimeline = timeline
    ? timeline.reduce<Array<{ kind: "segment"; segment: TurnSegment; index: number } | { kind: "tools"; tools: NonNullable<UIMessage["toolCalls"]>; startIndex: number; endIndex: number }>>((items, segment, index) => {
        if (segment.kind !== "tool") {
          items.push({ kind: "segment", segment, index });
          return items;
        }
        const tool = toolById.get(segment.toolCallId);
        if (isStreamingTail || !isQuietSuccess(tool)) {
          items.push({ kind: "segment", segment, index });
          return items;
        }
        const last = items[items.length - 1];
        if (last?.kind === "tools") {
          last.tools.push(tool!);
          last.endIndex = index;
        } else {
          items.push({ kind: "tools", tools: [tool!], startIndex: index, endIndex: index });
        }
        return items;
      }, [])
    : null;

  return (
    <div className="group text-sm text-gray-200 space-y-1.5">
      {msg.reviewProgress && (
        <div
          role="status"
          className="w-fit min-w-56 rounded-lg border border-sky-500/20 bg-sky-500/[0.06] px-3 py-2"
        >
          <div className="flex items-center justify-between gap-4">
            <span className="text-xs font-medium text-sky-700 dark:text-sky-300">
              {msg.reviewProgress.phase === "recovering"
                ? "正在补充验证"
                : msg.reviewProgress.phase === "finalizing"
                  ? "正在整理结果"
                  : "执行已中断"}
            </span>
            <span className="text-[10px] tabular-nums text-gray-500">
              第 {msg.reviewProgress.attempt}/{msg.reviewProgress.limit} 次
            </span>
          </div>
          <div className="mt-1 text-[11px] text-gray-500">
            {msg.reviewProgress.currentStep}
          </div>
          <div className="mt-1 flex items-center justify-between gap-4 text-[10px] text-gray-600">
            <span>{msg.reviewProgress.reason}</span>
            <span>
              最近活动 · {formatDuration(Math.max(0, nowMs - msg.reviewProgress.updatedAt))}前
            </span>
          </div>
        </div>
      )}
      {timeline ? (
        <>
          {collapsible && (
            <button
              type="button"
              onClick={() => setShowAllSteps((v) => !v)}
              className="text-[11px] text-gray-500 hover:text-gray-300 border border-surface-3 rounded px-2 py-0.5"
            >
              {showAllSteps ? "收起早期步骤" : `前 ${hiddenSteps} 步(点击展开)`}
            </button>
          )}
          {groupedTimeline!.map((item, itemIndex) => {
            if (item.kind === "tools") {
              if (item.endIndex < visibleFrom && !showAllSteps) return null;
              if (item.tools.length >= 3) return <SuccessfulToolGroup key={`tool-group-${itemIndex}`} tools={item.tools} />;
              return item.tools.map((tool) => <ToolCallCard key={`tool-${tool.id}`} tc={tool} />);
            }
            const { segment, index } = item;
            if (index < visibleFrom && !showAllSteps) return null;
            if (segment.kind === "tool") {
              const tc = toolById.get(segment.toolCallId);
              return tc ? <ToolCallCard key={`tool-${segment.toolCallId}`} tc={tc} /> : null;
            }
            const isFinal = index === lastTextIndex;
            if (!isFinal) {
              return (
                <div
                  key={`seg-${index}`}
                  data-segment="step"
                  className="border-l-2 border-surface-3 pl-2.5 py-0.5 text-[12px] leading-relaxed text-gray-400 whitespace-pre-wrap"
                >
                  {segment.text}
                </div>
              );
            }
            return (
              <div
                key={`seg-${index}`}
                data-segment="final"
                className="prose dark:prose-invert prose-sm max-w-none [&_pre]:!p-0 [&_pre]:!bg-transparent [&>*:first-child]:mt-0 [&>*:last-child]:mb-0"
              >
                <ReactMarkdown components={markdownComponents} remarkPlugins={[remarkGfm]}>{segment.text}</ReactMarkdown>
                {isStreamingTail && <TypingDots />}
              </div>
            );
          })}
        </>
      ) : (
        <>
          {(() => {
            const tools = msg.toolCalls ?? [];
            const items: Array<{ kind: "tools"; tools: typeof tools } | { kind: "tool"; tool: typeof tools[number] }> = [];
            for (const tool of tools) {
              if (!isStreamingTail && isQuietSuccess(tool)) {
                const last = items[items.length - 1];
                if (last?.kind === "tools") last.tools.push(tool);
                else items.push({ kind: "tools", tools: [tool] });
              } else {
                items.push({ kind: "tool", tool });
              }
            }
            return items.map((item, index) => {
              if (item.kind === "tool") return <ToolCallCard key={item.tool.id} tc={item.tool} />;
              if (item.tools.length >= 3) return <SuccessfulToolGroup key={`hydrated-tool-group-${index}`} tools={item.tools} />;
              return item.tools.map((tool) => <ToolCallCard key={tool.id} tc={tool} />);
            });
          })()}
        </>
      )}
      {msg.transportRetries?.map((retry, index) => (
        <div
          key={`${retry.attempt}-${index}`}
          className="w-fit max-w-full rounded border border-amber-500/30 bg-amber-500/10 px-2 py-1 text-[11px] leading-snug text-amber-800 dark:text-amber-200 break-words"
        >
          模型连接重试 {retry.attempt}/{retry.maxAttempts} · {retry.reason}
        </div>
      ))}
      {msg.gateActions
        ?.filter((action) => action.kind === "warning" || action.kind === "turn_notice")
        .map((action, index) => (
          <div
            key={`notice-${index}`}
            className={
              action.kind === "warning"
                ? "w-fit max-w-full rounded border border-amber-500/30 bg-amber-500/10 px-2 py-1 text-[11px] leading-snug text-amber-800 dark:text-amber-200 break-words"
                : "w-fit max-w-full rounded border border-sky-500/30 bg-sky-500/10 px-2 py-1 text-[11px] leading-snug text-sky-800 dark:text-sky-200 break-words"
            }
          >
            {action.detail}
          </div>
        ))}
      {!timeline && msg.content && (
        <div className="prose dark:prose-invert prose-sm max-w-none [&_pre]:!p-0 [&_pre]:!bg-transparent [&>*:first-child]:mt-0 [&>*:last-child]:mb-0">
          <ReactMarkdown components={markdownComponents} remarkPlugins={[remarkGfm]}>{msg.content}</ReactMarkdown>
          {isStreamingTail && <TypingDots />}
        </div>
      )}
      {showThinkingHint && (
        <div className="text-xs text-gray-500 inline-flex items-center">
          Thinking <TypingDots />
        </div>
      )}
      {durationLabel && (
        <div className="text-[10px] text-gray-600 tabular-nums select-none">
          {isStreamingTail ? `运行中 · ${durationLabel}` : `用时 ${durationLabel}`}
        </div>
      )}
      {showRemember && (
        <div className="flex justify-end pt-0.5">
          <RememberButton cwd={cwd} suggestedText={msg.content} />
        </div>
      )}
    </div>
  );
}
