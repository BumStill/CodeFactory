// SPDX-License-Identifier: Apache-2.0
import { useEffect, useRef } from "react";
import ReactMarkdown from "react-markdown";
import { ToolCallCard } from "./ToolCallCard";
import type { UIMessage } from "../stores/chat";

interface Props {
  messages: UIMessage[];
  streaming: boolean;
}

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
              <div className="prose prose-invert prose-sm max-w-none">
                <ReactMarkdown>{msg.content}</ReactMarkdown>
              </div>
            )}
          </div>
        )}
      </div>
    </div>
  );
}
