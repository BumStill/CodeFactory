// SPDX-License-Identifier: Apache-2.0
// Real-browser and real-WebView acceptance entry for timeline Markdown.

import React, { useState } from "react";
import { createRoot } from "react-dom/client";

import "../styles/globals.css";
import { MessageList } from "../components/MessageList";
import type { TurnSegment, UIMessage } from "../stores/chatEvents";

const formatted = [
  "**当前验证状态**",
  "",
  "- 已通过 `pnpm test`",
  "- [查看详情](https://example.com/check)",
].join("\n");

function buildMessage(continued: boolean, longTimeline: boolean): UIMessage {
  const segments: TurnSegment[] = [];
  const toolCalls: NonNullable<UIMessage["toolCalls"]> = [];
  const rounds = longTimeline ? 12 : 1;
  for (let index = 0; index < rounds; index += 1) {
    const text = index === 0 ? formatted : `**阶段 ${index + 1}**\n\n- 已完成 \`check-${index + 1}\``;
    segments.push({ kind: "text", text });
    if (continued || index < rounds - 1) {
      const toolCallId = `check-${index + 1}`;
      segments.push({ kind: "tool", toolCallId });
      toolCalls.push({
        id: toolCallId,
        name: "bash",
        args: JSON.stringify({ command: `pnpm test --filter ${toolCallId}` }),
        status: "done",
        result: "passed",
      });
    }
  }
  if (continued) segments.push({ kind: "text", text: "继续执行下一项。" });
  return {
    id: "streaming-markdown",
    role: "assistant",
    content: segments
      .filter((segment): segment is Extract<TurnSegment, { kind: "text" }> => segment.kind === "text")
      .map((segment) => segment.text)
      .join("\n"),
    createdAt: Date.now() - 4_000,
    segments,
    toolCalls,
  };
}

function AcceptanceApp() {
  const [continued, setContinued] = useState(false);
  const [longTimeline, setLongTimeline] = useState(false);
  const message = buildMessage(continued, longTimeline);
  return (
    <main
      className="min-h-screen bg-bg p-5 text-gray-200"
      aria-label="Streaming Markdown acceptance"
    >
      <header className="mx-auto mb-4 flex max-w-4xl flex-wrap items-center gap-2">
        <h1 className="mr-auto text-lg font-semibold">流式时间线 Markdown 验收</h1>
        <button
          type="button"
          className="rounded bg-accent px-3 py-1.5 text-xs text-white"
          onClick={() => setContinued(true)}
        >
          模拟工具与后续文本
        </button>
        <button
          type="button"
          className="rounded border border-border px-3 py-1.5 text-xs"
          onClick={() => {
            setLongTimeline(true);
            setContinued(true);
          }}
        >
          加载长时间线边界
        </button>
      </header>
      <section className="mx-auto flex h-[620px] max-w-4xl flex-col rounded-lg border border-border bg-surface-1">
        <MessageList
          messages={[message]}
          streaming
          cwd={null}
          conversationKey="streaming-markdown-acceptance"
        />
      </section>
    </main>
  );
}

createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <AcceptanceApp />
  </React.StrictMode>,
);
