// SPDX-License-Identifier: Apache-2.0
import { memo, useCallback, useEffect, useRef, useState } from "react";
import ReactMarkdown, { defaultUrlTransform } from "react-markdown";
import type { UrlTransform } from "react-markdown";
import { convertFileSrc } from "@tauri-apps/api/core";
import remarkGfm from "remark-gfm";
import type { Components } from "react-markdown";
import { createHighlighter, type Highlighter } from "shiki";
import {
  AlertTriangle,
  Check,
  Clock3,
  Copy,
  ChevronDown,
  ChevronUp,
  LoaderCircle,
  Sparkles,
  SquareTerminal,
} from "lucide-react";
import { ToolCallCard } from "./ToolCallCard";
import { ImagePreview } from "./ImagePreview";
import { FileArtifactCard, isDocumentPath } from "./FileArtifactCard";
import { WelcomeScreen } from "./WelcomeScreen";
import { useStickyAutoScroll } from "./useStickyAutoScroll";
import { ChatGptAuthRecovery } from "./ChatGptAuthRecovery";
import { formatDuration, formatElapsedClock, useNowTick } from "../lib/duration";
import { TurnProgress } from "./TurnProgress";
import { systemOwnsObjective } from "../lib/turnOwnership";
import { humanWaitingReason } from "../lib/waitingReason";
import {
  summarizeTurnEvidence,
  TurnResultSnapshot,
} from "./TurnResultSnapshot";
import type {
  ExternalJobState,
  TurnTimingProfile,
} from "../lib/chatPlan";
import type { UIMessage } from "../stores/chat";
import {
  isModelRouteExhaustedError,
  MODEL_ROUTE_EXHAUSTED_GUIDANCE,
  type TurnSegment,
} from "../stores/chatEvents";

interface Props {
  messages: UIMessage[];
  streaming: boolean;
  /** The session still owns the current turn, including durable recovery
   * between provider stream segments. Defaults to `streaming` for standalone
   * acceptance fixtures and older call sites. */
  turnActive?: boolean;
  /** Working directory of the active session. */
  cwd?: string | null;
  /** Called when the user picks an example prompt from the welcome screen. */
  onUsePrompt?: (text: string) => void;
  onOpenUsage?: () => void;
  /** Resume an existing conversation from the empty-state welcome screen. */
  onOpenSession?: (id: string) => void;
  /** Re-scope the current draft to a project directory (null = standalone). */
  onPickProject?: (cwd: string | null) => void;
  onOpenDocument?: (path: string) => void;
  onOpenEvidence?: (messageId: string) => void;
  evidenceControlsId?: string;
  openEvidenceMessageId?: string | null;
  conversationKey?: string | null;
  hasOlderHistory?: boolean;
  loadingOlderHistory?: boolean;
  historyTruncated?: boolean;
  onLoadOlder?: () => Promise<void>;
  timingProfile?: TurnTimingProfile | null;
  externalJobs?: ExternalJobState[];
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
    <div className="my-2 rounded-lg overflow-hidden border border-border bg-[#0d1117]">
      <div className="flex items-center justify-between px-3 py-1 bg-surface-3 border-b border-border">
        <span className="text-caption uppercase tracking-wide text-gray-500 font-sans">
          {lang === "text" ? "code" : lang}
        </span>
        <button
          onClick={handleCopy}
          className="flex min-h-7 items-center gap-1 rounded px-1.5 text-caption text-gray-500 transition-colors hover:bg-surface-4 hover:text-gray-200 font-sans"
          title="复制代码"
          aria-label={copied ? "已复制代码" : "复制代码"}
        >
          {copied ? <><Check size={14} className="text-status-success" /> 已复制</> : <><Copy size={14} /> 复制</>}
        </button>
      </div>
      {html ? (
        <div
          className="text-note overflow-x-auto [&>pre]:!p-3 [&>pre]:!bg-transparent"
          dangerouslySetInnerHTML={{ __html: html }}
        />
      ) : (
        <pre className="p-3 overflow-x-auto text-note leading-relaxed">
          <code>{code}</code>
        </pre>
      )}
    </div>
  );
}

const allowedLocalImageUrl: UrlTransform = (value, key, node): string => {
  if (node.tagName === "img" && key === "src" && value.startsWith("file://")) return value;
  return defaultUrlTransform(value);
};

function localFilePathFromUrl(url: string): string {
  const path = url.slice("file://".length);
  try {
    return decodeURI(path);
  } catch {
    return path;
  }
}

function previewImageSrc(src: string | undefined): string {
  if (!src) return "";
  if (src.startsWith("file://")) return convertFileSrc(localFilePathFromUrl(src));
  if (/^\/|^[A-Za-z]:[\\/]/.test(src)) return convertFileSrc(src);
  return src;
}


function normalizeLocalImageMarkdown(text: string): string {
  // react-markdown/commonmark treats whitespace inside a destination as the
  // end of the URL unless the destination is wrapped in <...>. Older persisted
  // messages and current send payloads store local image links as
  // `![name](file:///Users/me/Project With Spaces/.codefactory/attachments/x.png)`,
  // so normalize just those local-image links before markdown parsing.
  return text.replace(
    /!\[([^\]\n]*)\]\((file:\/\/[^)\n]+\.(?:png|jpe?g|gif|webp))\)/gi,
    (match, alt: string, url: string) => {
      if (!/\s/.test(url) || (url.startsWith("<") && url.endsWith(">"))) return match;
      return `![${alt}](<${url}>)`;
    },
  );
}

function MarkdownContent({ content, onOpenDocument }: { content: string; onOpenDocument?: (path: string) => void }) {
  const components = onOpenDocument
    ? {
        ...markdownComponents,
        code({ className, children, ...props }: { className?: string; children?: React.ReactNode }) {
          const code = String(children).replace(/\n$/, "");
          const match = /language-(\w+)/.exec(className || "");
          if (!match && isDocumentPath(code)) {
            return <FileArtifactCard path={code} compact onPreview={onOpenDocument} {...props} />;
          }
          if (match) return <CodeBlock lang={match[1]} code={code} />;
          return <code className="rounded bg-accent/10 px-1 py-0.5 font-mono text-note text-gray-300" {...props}>{children}</code>;
        },
      }
    : markdownComponents;
  return (
    <ReactMarkdown
      components={components}
      remarkPlugins={[remarkGfm]}
      urlTransform={allowedLocalImageUrl}
    >
      {normalizeLocalImageMarkdown(content)}
    </ReactMarkdown>
  );
}

function MarkdownImage({ src, alt }: { src?: string; alt?: string }) {
  const [failed, setFailed] = useState(false);
  const label = alt || "图片附件";
  const previewSrc = previewImageSrc(src);
  if (failed) {
    return (
      <span className="my-2 inline-flex max-w-full items-center gap-2 rounded-lg border border-dashed border-border/70 bg-surface-2 px-3 py-2 text-label text-gray-500 align-top">
        <span className="font-medium text-gray-400">图片预览失败</span>
        <span className="max-w-[220px] truncate font-mono text-caption">{label}</span>
      </span>
    );
  }
  return (
    <span className="my-2 inline-block max-w-full align-top">
      <ImagePreview
        src={previewSrc}
        alt={label}
        thumbnailClassName="max-h-80 max-w-full rounded-lg border border-border bg-surface-2 object-contain transition-opacity hover:opacity-90"
        caption={alt}
        onError={() => setFailed(true)}
      />
    </span>
  );
}

// ── Rich markdown component overrides ────────────────────────────────────────
const markdownComponents: Components = {
  code({ className, children, ...props }) {
    const match = /language-(\w+)/.exec(className || "");
    const isBlock = !!match;
    const code = String(children).replace(/\n$/, "");
    if (!isBlock && isDocumentPath(code)) {
      return <FileArtifactCard path={code} compact {...props} />;
    }
    if (isBlock) {
      return <CodeBlock lang={match![1]} code={code} />;
    }
    return (
      <code className="rounded bg-accent/10 px-1 py-0.5 font-mono text-note text-gray-300 ring-1 ring-inset ring-accent/10" {...props}>
        {children}
      </code>
    );
  },
  h1: ({ children }) => (
    <h1 className="mt-6 mb-3 pb-2 border-b border-border text-heading font-semibold text-gray-100">{children}</h1>
  ),
  h2: ({ children }) => (
    <h2 className="mt-5 mb-2 text-title font-semibold text-gray-100">{children}</h2>
  ),
  // The heading ladder never drops below the body it introduces. h3 and h4 used
  // to sit two steps down the old scale, rendering at 12.25px inside a 15px
  // message — the heading was smaller than the paragraph under it. At body size
  // they separate by weight instead, the way rendered Markdown normally does.
  h3: ({ children }) => (
    <h3 className="mt-4 mb-2 text-reading font-semibold text-gray-200">{children}</h3>
  ),
  h4: ({ children }) => (
    <h4 className="mt-3 mb-1.5 text-reading font-medium text-gray-300">{children}</h4>
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
      <table className="w-full border-collapse text-note">{children}</table>
    </div>
  ),
  thead: ({ children }) => <thead className="bg-surface-3">{children}</thead>,
  th: ({ children }) => (
    <th className="border border-border px-2 py-1 text-left font-semibold text-gray-200">{children}</th>
  ),
  td: ({ children }) => (
    <td className="border border-border px-2 py-1 text-gray-300">{children}</td>
  ),
  img: ({ src, alt }) => <MarkdownImage src={src} alt={alt} />,
  a: ({ children, href }) => (
    <a href={href} target="_blank" rel="noreferrer" className="text-accent hover:underline">
      {children}
    </a>
  ),
  strong: ({ children }) => <strong className="font-semibold text-gray-100">{children}</strong>,
  em: ({ children }) => <em className="italic text-gray-200">{children}</em>,
};

const TimelineMarkdownSegment = memo(function TimelineMarkdownSegment({
  text,
  isFinal,
}: {
  text: string;
  isFinal: boolean;
}) {
  return (
    <div
      data-segment={isFinal ? "final" : "step"}
      className={
        isFinal
          // The final answer is the reading stream — it must not be smaller
          // than the step narration above it. It used to carry no font size at
          // all and inherited the row's 14px, so a turn with tool calls showed
          // its mid-turn narration at 15px and its actual answer at 14px.
          ? "prose dark:prose-invert prose-sm max-w-none text-reading leading-6 [&_pre]:!p-0 [&_pre]:!bg-transparent [&>*:first-child]:mt-0 [&>*:last-child]:mb-0"
          : "prose dark:prose-invert prose-sm max-w-none py-0.5 text-reading leading-6 text-gray-300 [&_pre]:!p-0 [&_pre]:!bg-transparent [&_p]:my-1 [&_ul]:my-1 [&_ol]:my-1 [&_h1]:mt-2 [&_h2]:mt-2 [&_h3]:mt-1.5 [&_h4]:mt-1.5 [&>*:first-child]:mt-0 [&>*:last-child]:mb-0"
      }
    >
      <MarkdownContent content={text} />
    </div>
  );
});

// ── Main component ───────────────────────────────────────────────────────────
export function MessageList({
  messages,
  streaming,
  turnActive = streaming,
  onUsePrompt,
  onOpenUsage,
  onOpenSession,
  onPickProject,
  onOpenDocument,
  onOpenEvidence,
  evidenceControlsId,
  openEvidenceMessageId,
  conversationKey,
  hasOlderHistory = false,
  loadingOlderHistory = false,
  historyTruncated = false,
  onLoadOlder,
  timingProfile = null,
  externalJobs = [],
}: Props) {
  const resolvedConversationKey = conversationKey ?? messages[0]?.id ?? null;
  const contentSignal = messages.length === 0
    ? null
    : `${messages.length}:${messages[messages.length - 1]?.id ?? ""}:${turnActive ? "active" : "idle"}`;
  const {
    scrollerRef,
    pinned,
    hasNewContent,
    jumpToBottom,
    prepareForPrepend,
  } = useStickyAutoScroll(resolvedConversationKey, contentSignal);
  const conversationKeyRef = useRef(resolvedConversationKey);
  conversationKeyRef.current = resolvedConversationKey;
  const loadOlder = useCallback(async () => {
    const expectedConversationKey = conversationKeyRef.current;
    const anchor = prepareForPrepend();
    await onLoadOlder?.();
    requestAnimationFrame(() => {
      if (
        conversationKeyRef.current !== expectedConversationKey ||
        !anchor ||
        scrollerRef.current !== anchor.element
      ) {
        return;
      }
      anchor.restore();
    });
  }, [onLoadOlder, prepareForPrepend, scrollerRef]);

  if (messages.length === 0) {
    return (
      <div className="relative flex-1 min-h-0">
        <WelcomeScreen
          onUsePrompt={onUsePrompt}
          onOpenUsage={onOpenUsage}
          onOpenSession={onOpenSession}
          onPickProject={onPickProject}
        />
        {turnActive && (
          <div className="absolute bottom-3 left-1/2 w-[calc(100%-2rem)] max-w-[var(--reading-column)] -translate-x-1/2">
            <InlineTurnStatus
              message={undefined}
              streaming={streaming}
              startedAt={Date.now()}
            />
          </div>
        )}
      </div>
    );
  }

  const visible = messages.filter(
    (m) =>
      m.role !== "tool" &&
      m.completionState !== "rejected_candidate" &&
      (m.role !== "system" || m.completionState === "turn_notice"),
  );
  const lastAssistantId =
    [...visible].reverse().find((m) => m.role === "assistant")?.id ?? null;
  const activeProgressMessage = [...visible]
    .reverse()
    .find((message) => {
      if (
        message.turnActivity?.objectiveStatus &&
        !systemOwnsObjective(message.turnActivity.objectiveStatus)
      ) {
        return false;
      }
      if (streaming) {
        return message.role === "assistant" && Boolean(message.plan || message.turnActivity);
      }
      return systemOwnsObjective(message.turnActivity?.objectiveStatus);
    });
  const activeTurnMessage = turnActive
    ? [...visible]
        .reverse()
        .find((message) => message.role === "assistant")
    : undefined;
  const projectedActiveTurnMessage =
    activeTurnMessage?.turnActivity?.rootTurnId || activeTurnMessage?.plan?.rootTurnId
      ? activeTurnMessage
      : [...visible]
          .reverse()
          .find(
            (message) =>
              message.role === "assistant" &&
              Boolean(message.turnActivity?.rootTurnId) &&
              systemOwnsObjective(message.turnActivity?.objectiveStatus),
          );
  const activeRootTurnId =
    projectedActiveTurnMessage?.turnActivity?.rootTurnId ??
    projectedActiveTurnMessage?.plan?.rootTurnId;
  const activeRootMessage = activeRootTurnId
    ? visible.find(
        (message) => message.role === "user" && message.id === activeRootTurnId,
      )
    : [...visible].reverse().find((message) => message.role === "user");
  const activeTurnStartedAt =
    activeRootMessage?.createdAt ?? activeTurnMessage?.createdAt ?? Date.now();
  const lastAssistantIdsByUserTurn = new Set<string>();
  let pendingLastAssistantId: string | null = null;
  for (const message of visible) {
    if (message.role === "user") {
      if (pendingLastAssistantId) lastAssistantIdsByUserTurn.add(pendingLastAssistantId);
      pendingLastAssistantId = null;
    } else if (message.role === "assistant") {
      pendingLastAssistantId = message.id;
    }
  }
  if (pendingLastAssistantId) lastAssistantIdsByUserTurn.add(pendingLastAssistantId);

  return (
    <div className="relative flex-1 min-h-0">
      <div
        ref={scrollerRef}
        className="absolute inset-0 overflow-y-auto px-4 py-4"
      >
        <div
          data-testid="conversation-reading-column"
          className="mx-auto w-full max-w-[var(--reading-column)] pb-2"
        >
        {activeProgressMessage && (
          <div className="sticky top-0 z-20 mb-3 flex justify-center">
            <ActiveTurnProgress
              plan={activeProgressMessage.plan}
              activity={activeProgressMessage.turnActivity}
              startedAt={activeProgressMessage.createdAt}
              timingProfile={timingProfile}
              externalJobs={externalJobs}
            />
          </div>
        )}
        {hasOlderHistory && (
          <div className="flex justify-center">
            <button
              type="button"
              onClick={() => void loadOlder()}
              disabled={streaming || loadingOlderHistory}
              className="flex items-center gap-1 rounded-full border border-border bg-surface-2 px-2.5 py-1 text-caption text-gray-400 transition-colors hover:bg-surface-3 hover:text-gray-200 disabled:cursor-not-allowed disabled:opacity-50"
            >
              <ChevronUp size={14} />
              {loadingOlderHistory ? "正在加载更早记录" : "加载更早记录"}
            </button>
          </div>
        )}
        {historyTruncated && (
          <div
            role="status"
            className="mx-auto max-w-xl rounded-lg border border-status-warning/25 bg-status-warning-soft/55 px-3 py-2 text-center text-caption text-status-warning"
          >
            为保持超长会话可用，部分超大历史内容仅显示预览或分段加载；完整原始记录仍保存在本机。
          </div>
        )}
        {visible.map((msg, index) => {
          const previous = visible[index - 1];
          const messageFlow =
            msg.role === "user"
              ? "user-turn"
              : previous?.role === "assistant"
                ? "turn-continuation"
                : "turn-start";
          const spacing =
            index === 0
              ? ""
              : msg.role === "user"
                ? "mt-6"
                : messageFlow === "turn-continuation"
                  ? "mt-1"
                  : "mt-3";
          return (
          <div
            key={msg.id}
            data-message-row={msg.id}
            data-message-flow={messageFlow}
            className={spacing}
          >
            <MessageRow
              msg={msg}
              isStreamingTail={streaming && msg.id === lastAssistantId}
              isLastAssistantInUserTurn={lastAssistantIdsByUserTurn.has(msg.id)}
              onOpenDocument={onOpenDocument}
              onOpenEvidence={onOpenEvidence}
              evidenceControlsId={evidenceControlsId}
              evidenceOpen={openEvidenceMessageId === msg.id}
            />
          </div>
          );
        })}
        {turnActive && (
          <InlineTurnStatus
            message={activeTurnMessage}
            streaming={streaming}
            startedAt={activeTurnStartedAt}
          />
        )}
        </div>
      </div>

      {!pinned && (
        <button
          onClick={jumpToBottom}
          className={
            hasNewContent
              ? "absolute bottom-3 left-1/2 -translate-x-1/2 z-10 flex items-center gap-1.5 px-3 py-1.5 rounded-full border border-accent/60 bg-accent/15 text-accent text-caption font-medium shadow-lg hover:bg-accent/25 transition-colors animate-pulse motion-reduce:animate-none"
              : "absolute bottom-3 left-1/2 -translate-x-1/2 z-10 flex items-center gap-1 px-2.5 py-1 rounded-full border border-border bg-surface-2 text-caption text-gray-300 shadow-lg hover:bg-surface-3 transition-colors"
          }
          title={
            hasNewContent
              ? "有新内容，点击回到最新并恢复自动跟随"
              : "回到最新消息并恢复自动跟随"
          }
        >
          <ChevronDown size={14} />
          {hasNewContent ? "有新内容" : "回到最新"}
        </button>
      )}
    </div>
  );
}

type InlineTurnPhase = "thinking" | "executing" | "waiting" | "finalizing";

function inlineTurnPhase(
  message: UIMessage | undefined,
  streaming: boolean,
): InlineTurnPhase {
  const phase = message?.turnActivity?.phase;
  const kind = message?.turnActivity?.kind;
  if (phase === "finalizing" || kind === "finalizing") return "finalizing";

  const toolStatuses = (message?.toolCalls ?? []).map((tool) => tool.status);
  if (
    toolStatuses.some((status) => status === "waiting" || status === "waiting_permission")
  ) {
    return "waiting";
  }
  if (toolStatuses.includes("running")) return "executing";
  if (
    message?.turnActivity?.objectiveStatus === "waiting_system" ||
    !streaming
  ) {
    return "waiting";
  }
  return "thinking";
}

const INLINE_TURN_PHASE = {
  thinking: { label: "Thinking", Icon: LoaderCircle },
  executing: { label: "执行中", Icon: SquareTerminal },
  waiting: { label: "等待中", Icon: Clock3 },
  finalizing: { label: "整理结果", Icon: Sparkles },
} satisfies Record<InlineTurnPhase, { label: string; Icon: typeof LoaderCircle }>;

function InlineTurnStatus({
  message,
  streaming,
  startedAt,
}: {
  message: UIMessage | undefined;
  streaming: boolean;
  startedAt: number;
}) {
  const nowMs = useNowTick(true);
  const phase = inlineTurnPhase(message, streaming);
  const { label, Icon } = INLINE_TURN_PHASE[phase];
  const animated = phase === "thinking" || phase === "finalizing";

  return (
    <div
      data-testid="inline-turn-status"
      role="status"
      aria-live="polite"
      className="mt-1.5 inline-flex items-center gap-1.5 text-caption text-gray-500"
    >
      <Icon
        size={14}
        aria-hidden="true"
        className={`shrink-0 text-status-progress ${
          animated ? "animate-spin motion-reduce:animate-none" : ""
        }`}
      />
      <span>{label}</span>
      <span aria-hidden="true" className="tabular-nums text-gray-600 select-none">
        {" · "}{formatElapsedClock(Math.max(0, nowMs - startedAt))}
      </span>
    </div>
  );
}

function SuccessfulToolGroup({ tools }: { tools: NonNullable<UIMessage["toolCalls"]> }) {
  const [open, setOpen] = useState(false);
  return (
    <div data-tool-group="success" className="my-0.5 w-fit max-w-full">
      <button
        type="button"
        aria-label={`${open ? "收起" : "查看"} ${tools.length} 个已完成操作`}
        onClick={() => setOpen((value) => !value)}
        className="inline-flex min-h-7 max-w-full items-center gap-1.5 rounded-lg px-1.5 text-left text-note text-gray-600 transition-colors hover:bg-surface-3/55 hover:text-gray-400"
      >
        <Check size={14} className="text-status-success/70" />
        <span>已完成 {tools.length} 个操作</span>
        <ChevronDown size={14} className={`ml-auto transition-transform motion-reduce:transition-none ${open ? "rotate-180" : ""}`} />
      </button>
      {open && <div className="ml-2 border-l border-border/40 pl-2">{tools.map((tool) => <ToolCallCard key={tool.id} tc={tool} />)}</div>}
    </div>
  );
}

function ActiveTurnProgress({
  plan,
  activity,
  startedAt,
  timingProfile,
  externalJobs,
}: {
  plan?: UIMessage["plan"];
  activity?: UIMessage["turnActivity"];
  startedAt: number;
  timingProfile: TurnTimingProfile | null;
  externalJobs: ExternalJobState[];
}) {
  const nowMs = useNowTick(true);
  if (!plan) {
    const waitingReason = humanWaitingReason(activity?.waitingReason);
    const systemOwned = activity?.objectiveStatus === "active" || activity?.objectiveStatus === "waiting_system";
    const nextObservation = activity?.nextObservationAt
      ? Math.max(0, activity.nextObservationAt - nowMs)
      : null;
    return (
      <div
        role="status"
        data-testid="turn-activity-progress"
        data-status-tone={waitingReason ? "warning" : "progress"}
        className={`flex max-w-[min(34rem,calc(100vw-2rem))] items-center gap-2 rounded-full border bg-surface-2/95 px-3 py-1.5 text-caption shadow-lg backdrop-blur ${
          waitingReason
            ? "border-status-warning/35 text-status-warning"
            : "border-border text-gray-300"
        }`}
      >
        {waitingReason ? (
          <AlertTriangle size={14} aria-hidden="true" className="shrink-0" />
        ) : (
          <span className="h-2 w-2 shrink-0 animate-pulse rounded-full bg-status-progress motion-reduce:animate-none" />
        )}
        <span className="truncate">
          {systemOwned ? "系统仍在处理 · 恢复中" : activity?.label || "正在处理任务"}
        </span>
        {systemOwned && activity?.recoveryOwner && (
          <span className="max-w-[12rem] truncate text-gray-400">
            · {activity.recoveryOwner}
          </span>
        )}
        {systemOwned && activity?.label && (
          <span className="max-w-[16rem] truncate text-gray-400">
            · {activity.label}
          </span>
        )}
        {waitingReason && (
          <span className="max-w-[18rem] truncate text-status-warning/80">
            · {waitingReason}
          </span>
        )}
        {systemOwned && nextObservation !== null && (
          <span className="shrink-0 text-gray-500">
            · 下次观察 {formatDuration(nextObservation)} 后
          </span>
        )}
        <span className="shrink-0 text-gray-600">
          {formatDuration(Math.max(0, nowMs - startedAt))}
        </span>
      </div>
    );
  }
  return (
    <TurnProgress
      plan={plan}
      timingProfile={timingProfile}
      externalJobs={externalJobs}
      elapsedMs={Math.max(0, nowMs - startedAt)}
      nowMs={nowMs}
      activityLabel={activity?.kind === "tool_wait" ? activity.label : null}
      activityWaitingReason={activity?.waitingReason}
    />
  );
}

function isQuietSuccess(tool: NonNullable<UIMessage["toolCalls"]>[number] | undefined): boolean {
  return Boolean(tool && tool.status === "done" && !tool.isError);
}

function isModelRouteSwitchNotice(detail: string): boolean {
  return detail.includes("已自动切换到") && detail.includes("任务继续执行");
}

function isToolAmplificationNotice(detail: string): boolean {
  return detail.includes("本回合工具调用较多") && detail.includes("收敛剩余步骤");
}

const MessageRow = memo(function MessageRow({
  msg,
  isStreamingTail,
  isLastAssistantInUserTurn,
  onOpenDocument,
  onOpenEvidence,
  evidenceControlsId,
  evidenceOpen,
}: {
  msg: UIMessage;
  isStreamingTail: boolean;
  isLastAssistantInUserTurn: boolean;
  onOpenDocument?: (path: string) => void;
  onOpenEvidence?: (messageId: string) => void;
  evidenceControlsId?: string;
  evidenceOpen: boolean;
}) {
  const isUser = msg.role === "user";
  const [showAllSteps, setShowAllSteps] = useState(false);
  const turnBoundaryFailure = Boolean(
    msg.failureEvidence ||
    msg.runtimeError ||
    msg.turnActivity?.terminalReason,
  );

  // A persisted turn failure (provider error that killed the turn). Keep the
  // raw evidence across reloads without handing system-owned recovery back to
  // the user — the 2026-07-21 interruptions left zero trace because errors
  // were transient events.
  if (msg.completionState === "turn_error") {
    const persistedError = msg.content.replace(/^回合中断[::]\s*/, "");
    if (isModelRouteExhaustedError(persistedError)) {
      return (
        <section
          data-testid="failure-resolution-card"
          data-status-tone="warning"
          aria-label="系统仍在处理"
          className="max-w-[72ch] space-y-2 rounded-xl border border-status-warning/25 bg-status-warning-soft/55 px-3 py-2.5 text-note leading-5"
        >
          <div className="flex items-center gap-2">
            <AlertTriangle
              size={16}
              aria-hidden="true"
              className="shrink-0 text-status-warning"
            />
            <span className="font-semibold text-gray-200">系统仍在处理</span>
          </div>
          <p className="text-gray-300">
            {MODEL_ROUTE_EXHAUSTED_GUIDANCE}
          </p>
          <details className="border-t border-status-warning/20 pt-2 text-gray-500">
            <summary className="w-fit cursor-pointer select-none transition-colors hover:text-gray-300">
              查看失败详情
            </summary>
            <div className="mt-1 whitespace-pre-wrap break-words text-gray-500">
              {persistedError}
            </div>
          </details>
        </section>
      );
    }
    return (
      <section
        data-testid="failure-resolution-card"
        data-status-tone="danger"
        aria-label="回合中断"
        className="max-w-[72ch] space-y-1.5 rounded-xl border border-status-danger/25 bg-status-danger-soft/55 px-3 py-2.5 text-note leading-5"
      >
        <div className="flex items-center gap-2 font-semibold text-gray-200">
          <AlertTriangle
            size={16}
            aria-hidden="true"
            className="shrink-0 text-status-danger"
          />
          回合中断
        </div>
        <p className="break-words text-gray-300">{persistedError}</p>
      </section>
    );
  }

  if (msg.completionState === "auth_expired") {
    return (
      <div className="max-w-[72ch] space-y-2 rounded-lg border border-status-warning/30 bg-status-warning-soft/55 p-3">
        <p className="text-body font-medium text-gray-200">ChatGPT 授权已过期</p>
        <p className="text-label leading-5 text-gray-500">
          重新验证后系统会从已持久化的安全检查点自动继续，不会重复已确认的工具副作用。
        </p>
        <ChatGptAuthRecovery />
      </div>
    );
  }

  // Verification-incomplete warning: the reply stands, amber notice below.
  if (msg.completionState === "gate_warning") {
    return (
      <div className="flex justify-center">
        <div className="max-w-[85%] rounded border border-status-warning/30 bg-status-warning-soft/55 px-2.5 py-1 text-caption leading-snug text-status-warning break-words whitespace-pre-wrap">
          {msg.content}
        </div>
      </div>
    );
  }

  // Neutral runtime notices (e.g. images stripped for a no-vision model).
  if (msg.completionState === "turn_notice") {
    if (isModelRouteSwitchNotice(msg.content)) {
      return (
        <div
          role="status"
          aria-live="polite"
          className="text-note leading-5 text-gray-500 break-words"
        >
          {msg.content}
        </div>
      );
    }
    if (isToolAmplificationNotice(msg.content)) {
      return (
        <div className="flex justify-center">
          <div className="max-w-[85%] rounded border border-status-warning/30 bg-status-warning-soft/55 px-2.5 py-1 text-caption leading-snug text-status-warning break-words whitespace-pre-wrap">
            {msg.content}
          </div>
        </div>
      );
    }
    return (
      <div className="flex justify-center">
        <div className="max-w-[85%] rounded border border-status-info/30 bg-status-info-soft/55 px-2.5 py-1 text-caption leading-snug text-status-info break-words">
          {msg.content}
        </div>
      </div>
    );
  }

  // Gate prompts are framework instructions persisted as role=user so replayed
  // history stays faithful. They are not the user's words, so they stay out of
  // the transcript. A rejected draft, by contrast, is real model output and
  // renders like any other step in the turn.
  if (
    msg.completionState === "gate_recovery" ||
    msg.completionState === "gate_ready" ||
    msg.completionState === "gate_blocked"
  ) {
    return null;
  }

  if (isUser) {
    return (
      <div className="flex flex-col items-end">
        <div
          className={`max-w-[85%] bg-surface-3 rounded-2xl rounded-br-sm px-4 py-2.5 ${
            msg.steerPending ? "opacity-60" : ""
          }`}
        >
          <div className="prose dark:prose-invert prose-sm max-w-none whitespace-pre-wrap text-body text-gray-200 [&>*:first-child]:mt-0 [&>*:last-child]:mb-0">
            <MarkdownContent content={msg.content} onOpenDocument={onOpenDocument} />
          </div>
        </div>
        {/* Until a round boundary drains it the model genuinely has not seen
            this, and saying so is cheaper than letting the user wonder why
            nothing changed. */}
        {msg.steerPending && (
          <span className="mt-0.5 mr-1 text-caption text-gray-500">等待当前步骤结束…</span>
        )}
      </div>
    );
  }

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
  // Hydrated tool rounds are intermediate assistant narration, not separate
  // answers. Only a tool-free hydrated answer (or a live timeline ending in
  // prose) owns settled metadata such as elapsed time.
  const isSettledAnswer =
    !isStreamingTail &&
    isLastAssistantInUserTurn &&
    !!msg.content &&
    (timeline
      ? lastTextIndex === timeline.length - 1
      : !msg.toolCalls || msg.toolCalls.length === 0);
  const hasActiveTool = (msg.toolCalls ?? []).some(
    (tool) =>
      tool.status === "running" ||
      tool.status === "waiting" ||
      tool.status === "waiting_permission",
  );
  const isWaitingOnModelTransport = isStreamingTail && !hasActiveTool;
  const durationLabel = isSettledAnswer && msg.durationMs != null
      ? formatDuration(msg.durationMs)
      : null;
  // Keep the active turn as one continuous reading flow. Once the turn
  // reaches a terminal state, collapse its older segments behind the
  // existing disclosure so completed history stays compact.
  const COLLAPSE_THRESHOLD = 10;
  const TAIL_VISIBLE = 4;
  const collapsible =
    !isStreamingTail && (timeline?.length ?? 0) > COLLAPSE_THRESHOLD;
  const visibleFrom =
    collapsible && !showAllSteps ? (timeline?.length ?? 0) - TAIL_VISIBLE : 0;
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
    <div
      data-testid={msg.failureEvidence ? "failure-resolution-card" : undefined}
      data-status-tone={msg.failureEvidence ? "warning" : undefined}
      aria-label={msg.failureEvidence ? "失败证据" : undefined}
      className={`group space-y-1.5 text-body text-gray-200 ${
        msg.failureEvidence
          ? "max-w-[72ch] rounded-xl border border-status-warning/25 bg-status-warning-soft/55 px-3 py-2.5"
          : ""
      }`}
    >
      {msg.failureEvidence && (
        <div className="flex items-center gap-2">
          <AlertTriangle
            size={16}
            aria-hidden="true"
            className="shrink-0 text-status-warning"
          />
          <span className="text-note font-semibold text-gray-200">
            执行异常
          </span>
        </div>
      )}
      {msg.runtimeError?.code === "AUTH_EXPIRED" && (
        <div className="max-w-[72ch] space-y-2 rounded-lg border border-status-warning/30 bg-status-warning-soft/55 p-3">
          <p className="text-body font-medium text-gray-200">ChatGPT 授权已过期</p>
          <p className="text-label leading-5 text-gray-500">
            重新验证后可以回到这个会话继续；当前失败回合不会自动重放。
          </p>
          <ChatGptAuthRecovery />
        </div>
      )}
      {timeline ? (
        <>
          {collapsible && (
            <button
              type="button"
              aria-expanded={showAllSteps}
              aria-label={
                showAllSteps
                  ? "收起较早的执行过程"
                  : `展开较早的执行过程，共 ${hiddenSteps} 条`
              }
              onClick={() => setShowAllSteps((v) => !v)}
              className="inline-flex min-h-7 items-center gap-1 rounded-lg px-1.5 text-note leading-5 text-gray-500 transition-colors hover:bg-surface-3/55 hover:text-gray-300"
            >
              <ChevronDown
                size={14}
                aria-hidden="true"
                className={`transition-transform motion-reduce:transition-none ${showAllSteps ? "" : "-rotate-90"}`}
              />
              {showAllSteps ? "收起较早的执行过程" : "展开较早的执行过程"}
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
            return (
              <TimelineMarkdownSegment
                key={`seg-${index}`}
                text={segment.text}
                isFinal={isFinal}
              />
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
      {msg.transportRetries && msg.transportRetries.length > 0 && (() => {
        return (
          <details className="w-fit max-w-full text-note leading-5 text-gray-500">
            <summary className="cursor-pointer select-none hover:text-gray-400">
              {isWaitingOnModelTransport ? "模型连接不稳定，正在重新连接…" : "模型连接曾短暂不稳定，已完成重连"}
            </summary>
            <div className="ml-4 mt-1 space-y-0.5 text-note leading-5 text-gray-600">
              {msg.transportRetries.map((retry, index) => (
                <div key={`${retry.attempt}-${index}`} className="break-words">
                  第 {retry.attempt} 次 · {retry.reason}
                </div>
              ))}
            </div>
          </details>
        );
      })()}
      {msg.gateActions
        ?.filter((action) => action.kind === "warning" || action.kind === "turn_notice")
        .map((action, index) => (
          <div
            key={`notice-${index}`}
            role={isModelRouteSwitchNotice(action.detail) ? "status" : undefined}
            aria-live={isModelRouteSwitchNotice(action.detail) ? "polite" : undefined}
            className={
              action.kind === "warning"
                ? "w-fit max-w-full rounded border border-status-warning/30 bg-status-warning-soft/55 px-2 py-1 text-caption leading-snug text-status-warning break-words"
                : isModelRouteSwitchNotice(action.detail)
                  ? "text-note leading-5 text-gray-500 break-words"
                : "w-fit max-w-full rounded border border-status-info/30 bg-status-info-soft/55 px-2 py-1 text-caption leading-snug text-status-info break-words"
            }
          >
            {action.detail}
          </div>
        ))}
      {!timeline && msg.content ? (
        <div className="prose dark:prose-invert prose-sm max-w-none text-reading leading-6 [&_pre]:!p-0 [&_pre]:!bg-transparent [&>*:first-child]:mt-0 [&>*:last-child]:mb-0">
          <MarkdownContent content={msg.content} />
        </div>
      ) : null}
      {msg.failureEvidence && (
        <details className="mt-2 max-w-full border-t border-status-warning/20 pt-2 text-note leading-5 text-gray-500">
          <summary className="w-fit cursor-pointer select-none transition-colors hover:text-gray-300">
            查看失败详情
          </summary>
          <div className="mt-1 max-w-[72ch] whitespace-pre-wrap break-words text-gray-500">
            {msg.failureEvidence}
          </div>
        </details>
      )}
      {durationLabel && (
        <div className="text-caption text-gray-600 tabular-nums select-none">
          用时 {durationLabel}
        </div>
      )}
      {isSettledAnswer && msg.plan && (() => {
        const evidence = summarizeTurnEvidence(msg.turnToolCalls ?? msg.toolCalls ?? []);
        if (msg.turnToolCallCount != null) evidence.operationCount = msg.turnToolCallCount;
        return (
          <TurnResultSnapshot
            plan={msg.plan}
            evidence={evidence}
            turnBoundaryFailure={turnBoundaryFailure}
            durationMs={msg.durationMs ?? null}
            processExpanded={showAllSteps}
            onToggleProcess={
              collapsible ? () => setShowAllSteps((value) => !value) : undefined
            }
            onOpenEvidence={
              onOpenEvidence ? () => onOpenEvidence(msg.id) : undefined
            }
            evidenceControlsId={evidenceControlsId}
            evidenceOpen={evidenceOpen}
          />
        );
      })()}
    </div>
  );
});
