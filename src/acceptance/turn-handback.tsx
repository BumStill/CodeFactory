// SPDX-License-Identifier: Apache-2.0
// Real-browser acceptance entry for turn handback. It mounts the production
// MessageList with bounded fixtures so browser layout decides whether the
// progress banner is actually gone — jsdom renders no CSS, so a banner that
// merely moved or collapsed would still read as "absent" there.

import React from "react";
import { createRoot } from "react-dom/client";

import "../styles/globals.css";
import { MessageList } from "../components/MessageList";
import type { TurnPlan } from "../lib/chatPlan";
import type { UIMessage } from "../stores/chatEvents";

const plan: TurnPlan = {
  rootTurnId: "user",
  revision: 3,
  explanation: null,
  waitingReason: null,
  changeReason: null,
  createdAt: Date.now() - 9 * 60 * 1000,
  steps: [
    { id: "inspect", title: "确认输入区域蓝条的真实来源", kind: "analysis", status: "in_progress" },
    { id: "portal", title: "确认模型弹层脱位的触发场景", kind: "analysis", status: "pending" },
    { id: "report", title: "给出结论", kind: "other", status: "pending" },
  ],
};

// The turn this whole change came from: recovery exhausted, objective settled
// into waiting_core_input, nothing running, the user must type.
const handedBack: UIMessage[] = [
  { id: "user", role: "user", content: "只读分析输入框与弹层问题", createdAt: Date.now() - 9 * 60 * 1000 },
  {
    id: "assistant",
    role: "assistant",
    content: "分析结论如上，当前未修改代码。",
    createdAt: Date.now() - 10_000,
    durationMs: 6 * 60 * 1000,
    plan,
    turnActivity: {
      rootTurnId: "user",
      revision: 52,
      phase: "waiting",
      status: "waiting_core_input",
      kind: "technical_recovery_exhausted",
      label: "系统多轮自动恢复没有进展，已停止并把当前结论交还给你",
      waitingReason: "technical_recovery_exhausted",
      updatedAt: Date.now(),
      terminalReason: "technical_recovery_exhausted",
      objectiveId: "objective-handback",
      objectiveStatus: "waiting_core_input",
    } as UIMessage["turnActivity"],
  },
];

// The guard against over-suppression: a turn that really is running keeps its
// banner, its next step and its estimate.
const running: UIMessage[] = [
  { id: "user-2", role: "user", content: "跑一遍构建", createdAt: Date.now() - 4 * 60 * 1000 },
  {
    id: "assistant-2",
    role: "assistant",
    content: "正在构建。",
    createdAt: Date.now() - 3 * 60 * 1000,
    plan: { ...plan, rootTurnId: "user-2" },
    turnActivity: {
      rootTurnId: "user-2",
      revision: 8,
      phase: "working",
      status: "active",
      kind: "tool",
      label: "构建仍在运行",
      waitingReason: "命令已连续运行约 3 分钟",
      updatedAt: Date.now(),
      terminalReason: null,
      objectiveId: "objective-running",
      objectiveStatus: "active",
    } as UIMessage["turnActivity"],
  },
];

function Panel({ title, children }: { title: string; children: React.ReactNode }) {
  return (
    <section aria-label={title} style={{ height: 420, display: "flex", flexDirection: "column" }}>
      <h2 className="px-4 pt-3 text-note text-gray-300">{title}</h2>
      {children}
    </section>
  );
}

createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <main aria-label="Turn handback acceptance" className="flex flex-col gap-4 bg-surface-0">
      <Panel title="Handed back to the user">
        <MessageList messages={handedBack} streaming={false} cwd={null} />
      </Panel>
      <Panel title="Still running">
        <MessageList messages={running} streaming cwd={null} />
      </Panel>
    </main>
  </React.StrictMode>,
);
