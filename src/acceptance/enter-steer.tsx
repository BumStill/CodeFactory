// SPDX-License-Identifier: Apache-2.0
// Real-browser acceptance entry for M2. It mounts the production MessageInput
// and MessageList in the three states the task names — a live run, a turn busy
// in an uninterruptible tool, and a run that has already stalled — so a real
// layout engine decides what the composer promises before Enter and what it
// reports after Enter, and whether the conversation shows the pending steer.

import { useState } from "react";
import { createRoot } from "react-dom/client";

import "../styles/globals.css";
import { MessageInput } from "../components/MessageInput";
import { MessageList } from "../components/MessageList";
import type { UIMessage } from "../stores/chatEvents";

type Receipt = { delivery: { status: "delivered" | "queued"; into?: "new_turn" | "current_run" } };

const conversation: UIMessage[] = [
  {
    id: "assistant-live",
    role: "assistant",
    content: "正在执行第 2 步：修复输入框提示与真实发送语义。",
    createdAt: Date.now() - 60_000,
    rootTurnId: "root-1",
  },
  {
    id: "steer-pending",
    role: "user",
    content: "改用 chrome channel",
    createdAt: Date.now() - 1_000,
    steerPending: true,
  },
];

function Panel({
  name,
  hint,
  children,
}: {
  name: string;
  hint: string;
  children: React.ReactNode;
}) {
  return (
    <section aria-label={name} className="rounded-xl border border-border/60 bg-surface-1 p-3">
      <h2 className="mb-1 text-caption text-gray-500">{hint}</h2>
      {children}
    </section>
  );
}

function App() {
  const [theme, setTheme] = useState<"dark" | "light">("dark");
  const toggle = () => {
    const next = theme === "dark" ? "light" : "dark";
    document.documentElement.setAttribute("data-theme", next);
    document.documentElement.classList.toggle("dark", next === "dark");
    setTheme(next);
  };

  return (
    <main aria-label="Enter steering acceptance" className="min-h-screen bg-surface-0 p-4 text-gray-200">
      <button
        type="button"
        onClick={toggle}
        aria-label="切换主题"
        className="mb-3 rounded-lg border border-border/60 px-2 py-1 text-caption"
      >
        主题：{theme}
      </button>
      <div className="grid gap-3 lg:grid-cols-2">
        <Panel name="Live run" hint="一轮正在流式输出：Enter 引导当前执行">
          <MessageInput
            onSend={() => {}}
            onGuide={async (): Promise<Receipt> => {
              await new Promise((resolve) => setTimeout(resolve, 20));
              return { delivery: { status: "delivered", into: "current_run" } };
            }}
            onCancel={() => {}}
            streaming={true}
            guidanceActive={true}
            steeringActive={true}
            disabled={false}
            cwd="/proj"
          />
        </Panel>

        <Panel name="Tool blocked" hint="卡在不可中断的工具里：只能等当前步骤结束">
          <MessageInput
            onSend={() => {}}
            onGuide={async (): Promise<Receipt> => {
              await new Promise((resolve) => setTimeout(resolve, 20));
              return { delivery: { status: "queued" } };
            }}
            onCancel={() => {}}
            streaming={true}
            guidanceActive={true}
            steeringActive={true}
            hasPendingSteer={true}
            disabled={false}
            cwd="/proj"
          />
        </Panel>

        <Panel name="Stalled run" hint="回合已停摆：Enter 直接开新一轮">
          <MessageInput
            onSend={() => {}}
            onGuide={async (): Promise<Receipt> => ({ delivery: { status: "delivered", into: "new_turn" } })}
            onCancel={() => {}}
            streaming={false}
            guidanceActive={true}
            steeringActive={false}
            disabled={false}
            cwd="/proj"
          />
        </Panel>

        <Panel name="Conversation" hint="会话里可见等待状态">
          <MessageList messages={conversation} streaming={true} turnActive={true} turnExecutionActive={true} cwd="/proj" />
        </Panel>
      </div>
    </main>
  );
}

createRoot(document.getElementById("root")!).render(<App />);
