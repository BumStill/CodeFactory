// SPDX-License-Identifier: Apache-2.0
// Real-browser acceptance entry for turn handback. It mounts the production
// production reducer, MessageList, ToolCallCard and MessageInput with bounded
// fixtures so browser layout decides whether every in-flight surface actually
// converges after durable handback.

import React from "react";
import { createRoot } from "react-dom/client";

import "../styles/globals.css";
import { MessageList } from "../components/MessageList";
import { MessageInput } from "../components/MessageInput";
import type { TurnPlan } from "../lib/chatPlan";
import {
  reduceChatStreamEvent,
  type ChatEventState,
  type UIMessage,
} from "../stores/chatEvents";

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
const beforeHandback: ChatEventState = {
  streaming: true,
  inputTokenTotal: 0,
  outputTokenTotal: 0,
  pendingPermission: null,
  contextUsage: null,
  compressionToast: null,
  messages: [
  { id: "user", role: "user", content: "只读分析输入框与弹层问题", createdAt: Date.now() - 9 * 60 * 1000 },
  {
    id: "assistant",
    role: "assistant",
    content: "分析结论如上，当前未修改代码。",
    createdAt: Date.now() - 10_000,
    durationMs: 6 * 60 * 1000,
    inputTokens: 12_000,
    outputTokens: 345,
    plan,
    toolCalls: [{
      id: "audit-tool",
      name: "bash",
      args: "set -euo pipefail; git diff --check; git status --short",
      result: "external_state_uncertain",
      status: "waiting",
    }],
    segments: [{ kind: "tool", toolCallId: "audit-tool" }],
    turnActivity: {
      rootTurnId: "user",
      revision: 51,
      phase: "recovering",
      status: "active",
      kind: "tool",
      label: "正在恢复工具状态",
      waitingReason: "工具状态待确认",
      updatedAt: Date.now() - 1,
      terminalReason: null,
      objectiveId: "objective-handback",
      objectiveStatus: "waiting_system",
    } as UIMessage["turnActivity"],
  },
  ],
};

const handedBack = reduceChatStreamEvent(
  beforeHandback,
  {
    type: "turn_activity_updated",
    root_turn_id: "user",
    revision: 52,
    phase: "waiting",
    status: "waiting_core_input",
    recent_activity_kind: "technical_recovery_exhausted",
    recent_activity_label: "系统多轮自动恢复没有进展，已停止并把当前结论交还给你",
    waiting_reason: "technical_recovery_exhausted",
    updated_at: Date.now(),
    terminal_reason: "technical_recovery_exhausted",
    objective_id: "objective-handback",
    objective_status: "waiting_core_input",
  },
  "assistant",
);

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
      phase: "delivering",
      status: "active",
      kind: "tool_wait",
      label: "交付任务仍在运行（约 3 分钟）",
      waitingReason: "交付任务已连续运行约 3 分钟",
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

function Composer({ streaming }: { streaming: boolean }) {
  return (
    <div className="shrink-0 px-3 pb-3">
      <MessageInput
        onSend={() => {}}
        onCancel={() => {}}
        streaming={streaming}
        disabled={false}
        cwd={null}
      />
    </div>
  );
}

createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <main aria-label="Turn handback acceptance" className="flex flex-col gap-4 bg-surface-0">
      <Panel title="Handed back to the user">
        <MessageList messages={handedBack.messages} streaming={handedBack.streaming} cwd={null} />
        <Composer streaming={handedBack.streaming} />
      </Panel>
      <Panel title="Still running">
        <MessageList
          messages={running}
          streaming={false}
          turnActive
          turnExecutionActive
          cwd={null}
        />
        <Composer streaming />
      </Panel>
    </main>
  </React.StrictMode>,
);
