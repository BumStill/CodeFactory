// SPDX-License-Identifier: Apache-2.0
// Real-browser acceptance entry for terminal turn convergence. It mounts the production
// production reducer, MessageList, ToolCallCard and MessageInput with bounded
// fixtures so browser layout decides whether every in-flight surface actually
// converges after a system-owned incident.

import React from "react";
import { createRoot } from "react-dom/client";

import "../styles/globals.css";
import { MessageList } from "../components/MessageList";
import { MessageInput } from "../components/MessageInput";
import type { TurnPlan } from "../lib/chatPlan";
import { currentTurnOwnership } from "../lib/turnOwnership";
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

// Recovery exhausted: the current run settles into a system-owned incident.
// The objective remains durable and the user is not asked to type anything.
const beforeHandback: ChatEventState = {
  streaming: true,
  inputTokenTotal: 0,
  outputTokenTotal: 0,
  pendingPermission: null,
  contextUsage: null,
  compressionToast: null,
  messages: [
    {
      id: "user",
      role: "user",
      content: "只读分析输入框与弹层问题",
      createdAt: Date.now() - 9 * 60 * 1000,
    },
    {
      id: "assistant-before-steer",
      role: "assistant",
      rootTurnId: "user",
      content: "正在核对输入框与弹层状态。",
      createdAt: Date.now() - 8 * 60 * 1000,
      inputTokens: 12_000,
      outputTokens: 345,
      plan,
      toolCalls: [{
        id: "audit-tool-before-steer",
        name: "bash",
        args: "set -euo pipefail; git diff --check; git status --short",
        result: "external_state_uncertain",
        status: "waiting",
      }],
      segments: [
        { kind: "text", text: "正在核对输入框与弹层状态。" },
        { kind: "tool", toolCallId: "audit-tool-before-steer" },
      ],
      turnActivity: {
        rootTurnId: "user",
        revision: 51,
        phase: "recovering",
        status: "active",
        kind: "tool",
        label: "正在恢复工具状态",
        waitingReason: "工具状态待确认",
        updatedAt: Date.now() - 2,
        terminalReason: null,
        objectiveId: "objective-handback",
        objectiveStatus: "waiting_system",
      } as UIMessage["turnActivity"],
    },
    {
      id: "steer",
      role: "user",
      content: "继续完成，不需要我补充输入",
      createdAt: Date.now() - 7 * 60 * 1000,
    },
    {
      id: "assistant-after-steer",
      role: "assistant",
      rootTurnId: "user",
      content: "本回合的自动恢复已达到安全上限，已登记为系统故障。你不需要补充输入；CodeFactory 会在恢复策略或能力更新后续接同一目标。",
      createdAt: Date.now() - 10_000,
      toolCalls: [{
        id: "audit-tool-after-steer",
        name: "bash",
        args: "git log -1 --oneline",
        status: "running",
      }],
      segments: [
        {
          kind: "text",
          text: "本回合的自动恢复已达到安全上限，已登记为系统故障。你不需要补充输入；CodeFactory 会在恢复策略或能力更新后续接同一目标。",
        },
        { kind: "tool", toolCallId: "audit-tool-after-steer" },
      ],
    },
  ],
};

const incidentActivity = reduceChatStreamEvent(beforeHandback, {
    type: "turn_activity_updated",
    root_turn_id: "user",
    revision: 52,
    phase: "waiting",
    status: "waiting_system",
    recent_activity_kind: "technical_recovery_exhausted",
    recent_activity_label: "系统多轮自动恢复没有进展，已登记为系统故障；你不需要补充输入",
    waiting_reason: "technical_recovery_exhausted",
    updated_at: Date.now(),
    terminal_reason: "technical_recovery_exhausted",
    objective_id: "objective-handback",
    objective_status: "waiting_system",
  }, "assistant-before-steer");
const incident = reduceChatStreamEvent(incidentActivity, {
  type: "turn_settled",
  run_instance_id: "run-handback",
  root_turn_id: "user",
  objective_id: "objective-handback",
  status: "system_incident",
}, "assistant-before-steer");

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

function turnInFlight(messages: UIMessage[], streaming: boolean): boolean {
  const ownership = currentTurnOwnership(messages);
  return !ownership.released && (streaming || ownership.systemHeld);
}

const incidentInFlight = turnInFlight(incident.messages, incident.streaming);
const runningInFlight = turnInFlight(running, false);

createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <main aria-label="Turn terminal convergence acceptance" className="flex flex-col gap-4 bg-surface-0">
      <Panel title="System-owned incident">
        <MessageList
          messages={incident.messages}
          streaming={incident.streaming}
          turnActive={incidentInFlight}
          turnExecutionActive={incidentInFlight}
          cwd={null}
        />
        <Composer streaming={incidentInFlight} />
      </Panel>
      <Panel title="Still running">
        <MessageList
          messages={running}
          streaming={false}
          turnActive={runningInFlight}
          turnExecutionActive={runningInFlight}
          cwd={null}
        />
        <Composer streaming={runningInFlight} />
      </Panel>
    </main>
  </React.StrictMode>,
);
