// SPDX-License-Identifier: Apache-2.0
import { useEffect, useRef, useState } from "react";
import ReactMarkdown from "react-markdown";
import type { Components } from "react-markdown";
import { createHighlighter, type Highlighter } from "shiki";
import { ToolCallCard } from "./ToolCallCard";
import type { UIMessage } from "../stores/chat";

interface Props {
  messages: UIMessage[];
  streaming: boolean;
}

// Singleton highlighter — created once, reused across renders
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
      ],
    });
  }
  return _hlPromise;
}

function CodeBlock({ lang, code }: { lang: string; code: string }) {
  const [html, setHtml] = useState<string | null>(null);

  useEffect(() => {
    getHighlighter().then((hl) => {
      try {
        const resolved = hl.getLoadedLanguages().includes(lang as never) ? lang : "text";
        setHtml(hl.codeToHtml(code, { lang: resolved, theme: "github-dark" }));
      } catch {
        setHtml(null);
      }
    });
  }, [lang, code]);

  if (!html) {
    return (
      <pre className="bg-[#0d1117] rounded-md p-3 overflow-x-auto text-xs leading-relaxed">
        <code>{code}</code>
      </pre>
    );
  }

  return (
    <div
      className="rounded-md overflow-x-auto text-xs [&>pre]:p-3 [&>pre]:!bg-[#0d1117]"
      dangerouslySetInnerHTML={{ __html: html }}
    />
  );
}

const markdownComponents: Components = {
  code({ className, children, ...props }) {
    const match = /language-(\w+)/.exec(className || "");
    const isBlock = !!match;
    const code = String(children).replace(/\n$/, "");
    if (isBlock) {
      return <CodeBlock lang={match![1]} code={code} />;
    }
    return (
      <code className="bg-surface-3 px-1 py-0.5 rounded text-xs font-mono text-gray-200" {...props}>
        {children}
      </code>
    );
  },
};

export function MessageList({ messages, streaming }: Props) {
  const bottomRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    bottomRef.current?.scrollIntoView({ behavior: "smooth" });
  }, [messages]);

  if (messages.length === 0) {
    return (
      <div className="flex flex-1 items-center justify-center text-gray-600 select-none">
        <div className="text-center space-y-1">
          <div className="text-2xl font-semibold text-gray-500">CodeFactory</div>
          <div className="text-sm">Start a conversation or open a project folder</div>
        </div>
      </div>
    );
  }

  return (
    <div className="flex-1 overflow-y-auto px-4 py-4 space-y-4">
      {messages.filter((m) => m.role !== "tool" && m.role !== "system").map((msg) => (
        <MessageRow key={msg.id} msg={msg} />
      ))}
      {streaming && (
        <div className="flex items-center gap-1 text-gray-600 text-xs pl-1">
          <span className="animate-blink">▋</span>
        </div>
      )}
      <div ref={bottomRef} />
    </div>
  );
}

function MessageRow({ msg }: { msg: UIMessage }) {
  const isUser = msg.role === "user";

  return (
    <div className={`flex ${isUser ? "justify-end" : "justify-start"}`}>
      <div className={`max-w-[85%] ${isUser ? "bg-surface-3 rounded-2xl rounded-br-sm px-4 py-2.5" : ""}`}>
        {isUser ? (
          <p className="text-sm text-gray-200 whitespace-pre-wrap">{msg.content}</p>
        ) : (
          <div className="text-sm text-gray-200 space-y-1">
            {msg.toolCalls?.map((tc) => (
              <ToolCallCard key={tc.id} tc={tc} />
            ))}
            {msg.content && (
              <div className="prose prose-invert prose-sm max-w-none [&_pre]:!p-0 [&_pre]:!bg-transparent">
                <ReactMarkdown components={markdownComponents}>{msg.content}</ReactMarkdown>
              </div>
            )}
          </div>
        )}
      </div>
    </div>
  );
}
