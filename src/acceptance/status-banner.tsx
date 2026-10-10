// SPDX-License-Identifier: Apache-2.0
// Real-browser acceptance entry for the session status banner. It mounts the
// production MessageList component with synthetic fixtures so a real browser
// verifies the user-visible copy and layout of every banner state (CF-RSB-R1..R4),
// not just jsdom text.

import React from "react";
import { createRoot } from "react-dom/client";

import "../styles/globals.css";
import { MessageList } from "../components/MessageList";
import type { UIMessage } from "../stores/chatEvents";

type Activity = NonNullable<UIMessage["turnActivity"]>;

function activity(overrides: Partial<Activity>): Activity {
  return {
    rootTurnId: "user",
    revision: 3,
    phase: "working",
    status: "active",
    kind: "tool",
    label: "",
    waitingReason: null,
    updatedAt: Date.now(),
    terminalReason: null,
    ...overrides,
  };
}

function turn(overrides: Partial<Activity>): UIMessage[] {
  return [
    { id: "user", role: "user", content: "把这件事做完", createdAt: Date.now() - 25_000 },
    {
      id: "assistant",
      role: "assistant",
      content: "",
      createdAt: Date.now() - 24_000,
      turnActivity: activity(overrides),
    },
  ];
}

const fixtures: Array<{
  id: string;
  title: string;
  messages: UIMessage[];
  /** CF-HDE-R11: a live frontend stream is attached to this session. */
  streaming?: boolean;
  /** CF-HDE-R11: the server says a run currently owns this session. */
  sessionIsRunning?: boolean;
}> = [
  {
    id: "running",
    title: "正在执行（正常）",
    messages: turn({ objectiveStatus: "active", label: "正在执行命令" }),
  },
  {
    // CF-HDE-R11（M53）：真机证据——会话已经在模型上连续调用，活动投影却还停在
    // "等待自动重试"。只要这一轮真的在跑（实时流或服务器说在跑），横幅就不许再
    // 说"等待"。
    id: "live-stream-stale-waiting",
    title: "模型正在调用（陈旧的等待投影）",
    messages: turn({
      objectiveStatus: "waiting_system",
      nextObservationAt: Date.now() + 30_000,
      label: "正在调用模型",
    }),
    streaming: true,
  },
  {
    id: "server-running-stale-waiting",
    title: "服务器说会话在跑（陈旧的等待投影）",
    messages: turn({
      objectiveStatus: "waiting_system",
      nextObservationAt: Date.now() + 30_000,
      label: "正在调用模型",
    }),
    sessionIsRunning: true,
  },
  {
    id: "waiting",
    title: "等待重试（估计在未来）",
    messages: turn({ objectiveStatus: "waiting_system", nextObservationAt: Date.now() + 45_000 }),
  },
  {
    id: "overdue",
    title: "重试已过期（证据里的原始横幅）",
    messages: turn({
      objectiveStatus: "waiting_system",
      label: "已切换到备用模型 route",
      waitingReason: "等待退避窗口结束",
      recoveryOwner: "objective-supervisor:chat",
      nextObservationAt: Date.now() - 3_000,
    }),
  },
  {
    id: "unknown",
    title: "等待但没有估计",
    messages: turn({ objectiveStatus: "waiting_system", nextObservationAt: null }),
  },
  {
    id: "authorization",
    title: "需要用户授权",
    messages: turn({ objectiveStatus: "waiting_authorization", waitingReason: "authorization_required" }),
  },
  {
    id: "completed",
    title: "任务已结束（completed）",
    messages: turn({
      objectiveStatus: "completed",
      label: "系统仍在处理",
      terminalReason: "objective_completed",
    }),
  },
];

/// CF-RSB-R5: the exact banner recorded in the 2026-10-09 M36 evidence, kept as
/// a static "before" panel so one screenshot shows before and after together.
function BeforePanel() {
  return (
    <section
      data-fixture="before"
      aria-label="修复前的横幅（证据原串）"
      className="rounded-lg border border-status-warning/40 bg-status-warning-soft/40 p-3"
    >
      <h2 className="mb-3 text-body font-semibold text-gray-200">修复前（2026-10-09 M36 证据原串）</h2>
      <div
        role="status"
        className="inline-flex max-w-full items-center gap-2 rounded-full border border-border bg-surface-2 px-3 py-1.5 text-caption text-gray-300"
      >
        <span className="truncate">
          系统仍在处理 · 恢复中 · objective-supervisor:chat · 正在执行命令 · 下次观察 0ms 后 · 25.0s
        </span>
      </div>
    </section>
  );
}

function Panel({ title, children }: { title: string; children: React.ReactNode }) {
  return (
    <section className="min-h-[220px] rounded-lg border border-border bg-surface-1 p-3">
      <h2 className="mb-3 text-body font-semibold text-gray-200">{title}</h2>
      <div className="h-[180px] rounded border border-border bg-bg">
        {children}
      </div>
    </section>
  );
}

function AcceptanceApp() {
  return (
    <main className="min-h-screen bg-bg p-6 text-gray-200" aria-label="Status banner acceptance">
      <h1 className="mb-4 text-heading font-semibold">会话顶部状态横幅验收</h1>
      <div className="mb-4">
        <BeforePanel />
      </div>
      <div className="grid gap-4 md:grid-cols-2">
        {fixtures.map((fixture) => (
          <Panel key={fixture.id} title={fixture.title}>
            <div data-fixture={fixture.id} className="h-full">
              <MessageList
                messages={fixture.messages}
                streaming={fixture.streaming ?? fixture.id === "running"}
                sessionIsRunning={fixture.sessionIsRunning === true}
                cwd={null}
                conversationKey={`status-banner-${fixture.id}`}
              />
            </div>
          </Panel>
        ))}
      </div>
    </main>
  );
}

createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <AcceptanceApp />
  </React.StrictMode>,
);
