// SPDX-License-Identifier: Apache-2.0
// Real-browser acceptance entry for reconnect banner attribution. It mounts
// the production MessageList component with bounded fixtures so browser layout
// verifies the user-visible copy, not just jsdom text.

import React from "react";
import { createRoot } from "react-dom/client";

import "../styles/globals.css";
import { MessageList } from "../components/MessageList";
import type { UIMessage } from "../stores/chatEvents";

const retry = {
  label: "OpenAI-compatible chat stream request",
  attempt: 1,
  maxAttempts: 3,
  delayMs: 300,
  reason: "HTTP 503 Service Unavailable",
};

const runningToolMessage: UIMessage = {
  id: "tool-running-tail",
  role: "assistant",
  content: "CI 轮询超时但无失败，只剩 `check` 在跑；我继续低频查询。",
  createdAt: Date.now() - 24 * 60 * 1000,
  transportRetries: [retry],
  segments: [
    { kind: "text", text: "CI 轮询超时但无失败，只剩 `check` 在跑；我继续低频查询。" },
    { kind: "tool", toolCallId: "ci-poll" },
  ],
  toolCalls: [
    {
      id: "ci-poll",
      name: "bash",
      args: JSON.stringify({ command: "poll pull request checks" }),
      status: "running",
      result: "attempt=1 count=5 pending=3 failed=0",
    },
  ],
};

const modelReconnectMessage: UIMessage = {
  id: "model-waiting-tail",
  role: "assistant",
  content: "",
  createdAt: Date.now() - 5_000,
  transportRetries: [retry],
};

function Panel({ title, children }: { title: string; children: React.ReactNode }) {
  return (
    <section className="min-h-[260px] rounded-lg border border-border bg-surface-1 p-3">
      <h2 className="mb-3 text-body font-semibold text-gray-200">{title}</h2>
      <div className="h-[220px] rounded border border-border bg-bg">
        {children}
      </div>
    </section>
  );
}

function AcceptanceApp() {
  return (
    <main className="min-h-screen bg-bg p-6 text-gray-200" aria-label="Reconnect banner acceptance">
      <h1 className="mb-4 text-heading font-semibold">Reconnect banner attribution acceptance</h1>
      <div className="grid gap-4 md:grid-cols-2">
        <Panel title="Tool command is still running">
          <MessageList messages={[runningToolMessage]} streaming cwd={null} conversationKey="tool-running" />
        </Panel>
        <Panel title="Actually waiting on model transport">
          <MessageList messages={[modelReconnectMessage]} streaming cwd={null} conversationKey="model-waiting" />
        </Panel>
      </div>
    </main>
  );
}

createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <AcceptanceApp />
  </React.StrictMode>,
);
