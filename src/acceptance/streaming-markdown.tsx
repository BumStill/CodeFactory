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

type SemanticPhase = "active" | "waiting" | "settled" | "history";

function buildSemanticMessage(phase: SemanticPhase): UIMessage {
  const routineTools = Array.from({ length: 12 }, (_, index) => ({
    id: `semantic-read-${index}`,
    name: index < 8 ? "read_file" : index < 10 ? "grep" : "list_files",
    args: index === 0
      ? JSON.stringify({ path: "fixtures/API_TOKEN=SECRET-never-summarize.ts" })
      : JSON.stringify({ path: `src/semantic-${index}.ts`, pattern: "collapse" }),
    status: "done" as const,
    result: index === 0 ? "SECRET-output-never-summarize" : "ok",
  }));
  const toolCalls: NonNullable<UIMessage["toolCalls"]> = [
    ...routineTools,
    {
      id: "semantic-edit",
      name: "edit_file",
      args: JSON.stringify({ path: "src/components/MessageList.tsx" }),
      status: "done",
      result: "updated",
    },
    {
      id: "semantic-test",
      name: "bash",
      args: JSON.stringify({ command: "pnpm test --filter MessageList" }),
      status: "done",
      result: "18 passed",
    },
    {
      id: "semantic-failure",
      name: "bash",
      args: JSON.stringify({ command: "pnpm check --first-attempt" }),
      status: "error",
      result: "first attempt failed harmlessly",
      isError: true,
    },
    {
      id: "semantic-unknown",
      name: "custom_probe",
      args: "{}",
      status: "done",
      result: "material finding",
    },
  ];
  const segments: TurnSegment[] = [
    { kind: "text", text: "根因：固定位置截断把较早判断和证据一起隐藏了。" },
    ...routineTools.map((tool): TurnSegment => ({ kind: "tool", toolCallId: tool.id })),
    { kind: "text", text: "决策：只收束例行读取，编辑、验证和异常必须直接可见。" },
    { kind: "tool", toolCallId: "semantic-edit" },
    { kind: "tool", toolCallId: "semantic-test" },
    { kind: "tool", toolCallId: "semantic-failure" },
    { kind: "tool", toolCallId: "semantic-unknown" },
    { kind: "text", text: "最终结论：完成时保持完整，进入历史后才语义紧凑。" },
  ];
  const objectiveStatus = phase === "waiting"
    ? "waiting_system"
    : phase === "active"
      ? "active"
      : "completed";
  return {
    id: "semantic-collapse",
    role: "assistant",
    content: "最终结论：完成时保持完整，进入历史后才语义紧凑。",
    createdAt: Date.now() - 8_000,
    segments,
    toolCalls,
    turnActivity: {
      rootTurnId: "semantic-root",
      revision: 1,
      phase: phase === "waiting" ? "recovery" : "verification",
      status: phase === "waiting" ? "waiting" : phase === "active" ? "running" : "completed",
      kind: phase === "waiting" ? "tool_wait" : "progress",
      label: phase === "waiting" ? "系统正在恢复" : "验证语义过程呈现",
      waitingReason: phase === "waiting" ? "等待恢复服务" : null,
      updatedAt: Date.now(),
      terminalReason: null,
      objectiveStatus,
    },
  };
}

function AcceptanceApp() {
  const [continued, setContinued] = useState(false);
  const [longTimeline, setLongTimeline] = useState(false);
  const [semanticPhase, setSemanticPhase] = useState<SemanticPhase | null>(null);
  const [lightTheme, setLightTheme] = useState(false);
  const message = semanticPhase
    ? buildSemanticMessage(semanticPhase)
    : buildMessage(continued, longTimeline);
  const messages: UIMessage[] = semanticPhase === "history"
    ? [
        message,
        {
          id: "semantic-next-user",
          role: "user",
          content: "继续处理下一件事。",
          createdAt: Date.now(),
        },
      ]
    : [message];
  const semanticActive = semanticPhase === "active";
  return (
    <main
      className="min-h-screen bg-bg p-5 text-gray-200"
      aria-label="Streaming Markdown acceptance"
    >
      <header className="mx-auto mb-4 flex max-w-4xl flex-wrap items-center gap-2">
        <h1 className="mr-auto text-heading font-semibold">流式时间线 Markdown 验收</h1>
        <button
          type="button"
          className="rounded bg-accent px-3 py-1.5 text-label text-white"
          onClick={() => setContinued(true)}
        >
          模拟工具与后续文本
        </button>
        <button
          type="button"
          className="rounded border border-border px-3 py-1.5 text-label"
          onClick={() => {
            setLongTimeline(true);
            setContinued(true);
          }}
        >
          加载长时间线边界
        </button>
        <button
          type="button"
          className="rounded border border-border px-3 py-1.5 text-label"
          onClick={() => setSemanticPhase("active")}
        >
          加载语义长回合
        </button>
        <button
          type="button"
          disabled={!semanticPhase}
          className="rounded border border-border px-3 py-1.5 text-label disabled:opacity-40"
          onClick={() => setSemanticPhase("waiting")}
        >
          模拟系统等待
        </button>
        <button
          type="button"
          disabled={!semanticPhase}
          className="rounded border border-border px-3 py-1.5 text-label disabled:opacity-40"
          onClick={() => setSemanticPhase("settled")}
        >
          模拟完成
        </button>
        <button
          type="button"
          disabled={semanticPhase !== "settled"}
          className="rounded border border-border px-3 py-1.5 text-label disabled:opacity-40"
          onClick={() => setSemanticPhase("history")}
        >
          追加下一用户回合
        </button>
        <button
          type="button"
          className="rounded border border-border px-3 py-1.5 text-label"
          onClick={() => {
            const next = !lightTheme;
            setLightTheme(next);
            document.documentElement.setAttribute("data-theme", next ? "light" : "dark");
          }}
        >
          {lightTheme ? "切换深色主题" : "切换浅色主题"}
        </button>
      </header>
      <section className="mx-auto flex h-[calc(100vh-8rem)] min-h-[360px] max-w-4xl flex-col rounded-lg border border-border bg-surface-1">
        <MessageList
          messages={messages}
          streaming={semanticPhase ? semanticActive : true}
          turnActive={semanticPhase ? semanticActive : true}
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
