// SPDX-License-Identifier: Apache-2.0
// CF-BG 阶段 3（R6）真实浏览器验收页：会话头部的小标记 + 展开面板。
//
// jsdom 不计算布局，所以这里挂载**生产组件**，用合成数据（不写生产 DB）在真实
// layout engine 里检查：标记文案与数量、展开后的命令/来源/状态、停止与取消入口、
// 以及明暗两种主题和窄屏下不遮挡不溢出。

import { createRoot } from "react-dom/client";
import "../styles/globals.css";
import { SessionBackgroundTasks } from "../components/SessionBackgroundTasks";
import type { SessionBackgroundActivity } from "../lib/backgroundTasks";

const activity: SessionBackgroundActivity = {
  tasks: [
    {
      id: "bg-1",
      session_id: "session-273a55b2",
      command: "gh pr checks 742 --watch",
      cwd: "/repo",
      source: "agent_decided",
      status: "running",
      exit_code: null,
      created_at: 1,
      finished_at: null,
      timeout_sec: 10_800,
    },
    {
      id: "bg-2",
      session_id: "session-273a55b2",
      command: "pnpm test",
      cwd: "/repo",
      source: "user_requested",
      status: "exited",
      exit_code: 0,
      created_at: 2,
      finished_at: 3,
      timeout_sec: 600,
    },
  ],
  wakeups: [
    {
      id: "wk-1",
      session_id: "session-273a55b2",
      prompt: "盯着 PR #742，合并了告诉我",
      source: "user_requested",
      after_sec: null,
      every_sec: 180,
      next_at: 10,
      status: "pending",
      fire_count: 0,
    },
  ],
};

const loadActivity = async () => activity;
const stopTask = async () => {};
const cancelWakeup = async () => {};

function Panel({ theme }: { theme: "light" | "dark" }) {
  return (
    <section
      aria-label={`${theme} 会话头部`}
      className={theme === "dark" ? "dark" : undefined}
      data-theme={theme}
    >
      <div className="flex items-center justify-between gap-2 border-b border-border bg-surface px-3 py-2">
        <span className="text-note text-text-secondary">会话 273a55b2</span>
        <SessionBackgroundTasks
          sessionId="session-273a55b2"
          loadActivity={loadActivity}
          stopTask={stopTask}
          cancelWakeup={cancelWakeup}
        />
      </div>
    </section>
  );
}

function App() {
  return (
    <main aria-label="Session background tasks acceptance" className="min-h-screen bg-background p-4">
      <div className="mx-auto grid max-w-5xl gap-6">
        <Panel theme="light" />
        <Panel theme="dark" />
      </div>
    </main>
  );
}

createRoot(document.getElementById("root")!).render(<App />);
