// SPDX-License-Identifier: Apache-2.0
//! CF-BG 阶段 3（R6）：会话内后台任务/定时唤醒的界面数据源。
//!
//! 数据全部来自后端同一份 `background_task` / `wakeup_schedules` 记录，因此界面
//! 显示的"进行中"就是 agent 工具看到的同一行；这里不缓存、不推断、不伪造状态。

import { invoke } from "@tauri-apps/api/core";

export type BackgroundTaskStatus =
  | "running"
  | "exited"
  | "failed"
  | "timed_out"
  | "stopped"
  | "lost";

export type BackgroundTaskSource = "agent_decided" | "user_requested";

export interface BackgroundTaskRow {
  id: string;
  session_id: string;
  command: string;
  cwd: string;
  source: BackgroundTaskSource;
  status: BackgroundTaskStatus;
  exit_code: number | null;
  created_at: number;
  finished_at: number | null;
  timeout_sec: number;
}

export interface WakeupRow {
  id: string;
  session_id: string;
  prompt: string;
  source: BackgroundTaskSource;
  after_sec: number | null;
  every_sec: number | null;
  next_at: number;
  status: string;
  fire_count: number;
}

export interface SessionBackgroundActivity {
  tasks: BackgroundTaskRow[];
  wakeups: WakeupRow[];
}

export const BACKGROUND_STATUS_LABEL: Record<BackgroundTaskStatus, string> = {
  running: "运行中",
  exited: "已结束",
  failed: "失败",
  timed_out: "超时",
  stopped: "已停止",
  lost: "状态未知",
};

export function backgroundSourceLabel(source: BackgroundTaskSource): string {
  return source === "user_requested" ? "按你的要求" : "由 agent 启动";
}

export function wakeupCadence(wakeup: WakeupRow): string {
  return wakeup.every_sec
    ? `每 ${wakeup.every_sec} 秒`
    : `一次性（${wakeup.after_sec ?? 0} 秒后）`;
}

export async function listSessionBackgroundActivity(
  sessionId: string,
): Promise<SessionBackgroundActivity> {
  const activity = await invoke<SessionBackgroundActivity | null>(
    "list_session_background_activity",
    { sessionId },
  );
  return {
    tasks: activity?.tasks ?? [],
    wakeups: activity?.wakeups ?? [],
  };
}

export async function stopSessionBackgroundTask(
  sessionId: string,
  taskId: string,
): Promise<void> {
  await invoke("stop_session_background_task", { sessionId, taskId });
}

export async function cancelSessionWakeup(
  sessionId: string,
  wakeupId: string,
): Promise<void> {
  await invoke("cancel_session_wakeup", { sessionId, wakeupId });
}
