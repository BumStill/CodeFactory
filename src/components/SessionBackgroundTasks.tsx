// SPDX-License-Identifier: Apache-2.0
// CF-BG 阶段 3（R6）：会话头部的小标记「N 个后台任务进行中」。
//
// 只读展示 + 停止入口；唤醒和控制回路不写进消息流，也不产生聊天消息。
import { useCallback, useEffect, useRef, useState } from "react";
import { Loader2, Square, Timer, X } from "lucide-react";

import {
  BACKGROUND_STATUS_LABEL,
  backgroundSourceLabel,
  cancelSessionWakeup,
  listSessionBackgroundActivity,
  SILENT_COMMAND_SEC,
  stopSessionBackgroundTask,
  wakeupCadence,
  type SessionBackgroundActivity,
} from "../lib/backgroundTasks";

const POLL_MS = 5_000;

function silentSeconds(lastOutputAt: number): number {
  return Math.max(0, Math.floor(Date.now() / 1000) - lastOutputAt);
}

function silentMinutes(lastOutputAt: number): number {
  return Math.max(1, Math.round(silentSeconds(lastOutputAt) / 60));
}

interface Props {
  sessionId: string | null;
  /** 验收页可注入合成数据；生产路径始终用真实命令。 */
  loadActivity?: typeof listSessionBackgroundActivity;
  stopTask?: typeof stopSessionBackgroundTask;
  cancelWakeup?: typeof cancelSessionWakeup;
}

export function SessionBackgroundTasks({
  sessionId,
  loadActivity = listSessionBackgroundActivity,
  stopTask: stopTaskCommand = stopSessionBackgroundTask,
  cancelWakeup: cancelWakeupCommand = cancelSessionWakeup,
}: Props) {
  const [activity, setActivity] = useState<SessionBackgroundActivity>({
    tasks: [],
    wakeups: [],
    commands: [],
  });
  const [expanded, setExpanded] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const mounted = useRef(true);

  const refresh = useCallback(async () => {
    if (!sessionId) {
      if (mounted.current) setActivity({ tasks: [], wakeups: [], commands: [] });
      return;
    }
    try {
      const next = await loadActivity(sessionId);
      if (mounted.current) {
        setActivity(next);
        setError(null);
      }
    } catch (cause) {
      if (mounted.current) setError(String(cause));
    }
  }, [sessionId, loadActivity]);

  useEffect(() => {
    mounted.current = true;
    void refresh();
    const timer = window.setInterval(() => void refresh(), POLL_MS);
    return () => {
      mounted.current = false;
      window.clearInterval(timer);
      setExpanded(false);
    };
  }, [refresh]);

  const running = activity.tasks.filter((task) => task.status === "running");
  // 只把真的超过阈值没有输出的前台命令算进来：仍在持续输出的慢命令不打扰用户。
  const silent = activity.commands.filter(
    (command) => silentSeconds(command.last_output_at) >= SILENT_COMMAND_SEC,
  );

  const stopCommand = async (commandId: string) => {
    if (!sessionId) return;
    try {
      await stopTaskCommand(sessionId, commandId);
      await refresh();
    } catch (cause) {
      setError(String(cause));
    }
  };

  const stopTask = async (taskId: string) => {
    if (!sessionId) return;
    try {
      await stopTaskCommand(sessionId, taskId);
      await refresh();
    } catch (cause) {
      setError(String(cause));
    }
  };

  const cancelWakeup = async (wakeupId: string) => {
    if (!sessionId) return;
    try {
      await cancelWakeupCommand(sessionId, wakeupId);
      await refresh();
    } catch (cause) {
      setError(String(cause));
    }
  };

  if (!sessionId) return null;
  // 没有进行中的任务/唤醒时不占位置：这个标记只在真有后台工作时出现。
  if (running.length === 0 && activity.wakeups.length === 0 && silent.length === 0) return null;

  const liveCount = running.length;
  const label = liveCount > 0
    ? `${liveCount} 个后台任务进行中`
    : silent.length > 0
      ? `${silent.length} 个命令长时间无输出`
      : `${activity.wakeups.length} 个定时唤醒待触发`;

  return (
    <div className="relative">
      <button
        type="button"
        aria-expanded={expanded}
        aria-label={`后台活动：${label}`}
        title={error ? `${label}（读取失败）` : label}
        onClick={() => setExpanded((open) => !open)}
        className="flex min-h-11 items-center gap-1 rounded-lg border border-border bg-surface px-2 text-note text-text-secondary transition-colors hover:bg-surface-hover lg:min-h-9"
      >
        <Loader2
          size={14}
          className={liveCount > 0 || silent.length > 0 ? "animate-spin" : undefined}
        />
        <span className="max-w-[10rem] truncate">{label}</span>
      </button>

      {expanded && (
        <div
          role="region"
          aria-label="后台任务详情"
          className="absolute right-0 top-full z-30 mt-1 w-[min(22rem,calc(100vw-2rem))] rounded-xl border border-border bg-surface p-2 shadow-lg"
        >
          {silent.length > 0 && (
            <ul className="space-y-1">
              {silent.map((command) => (
                <li key={command.id} className="rounded-lg px-2 py-1.5 hover:bg-surface-hover">
                  <div className="flex items-start gap-2">
                    <div className="min-w-0 flex-1">
                      <p
                        className="truncate font-mono text-note text-text-primary"
                        title={command.command}
                      >
                        {command.command}
                      </p>
                      <p className="mt-0.5 text-caption text-text-tertiary">
                        卡在 {command.command} 已 {silentMinutes(command.last_output_at)} 分钟，无输出
                      </p>
                    </div>
                    <button
                      type="button"
                      aria-label={`停止命令 ${command.command}`}
                      onClick={() => void stopCommand(command.id)}
                      className="flex min-h-9 items-center gap-1 rounded-lg border border-border px-1.5 text-caption text-text-secondary hover:bg-surface"
                    >
                      <Square size={14} /> 停止
                    </button>
                  </div>
                </li>
              ))}
            </ul>
          )}

          {activity.tasks.length === 0 ? (
            silent.length === 0 && (
              <p className="px-2 py-1 text-note text-text-tertiary">没有后台任务记录</p>
            )
          ) : (
            <ul className="space-y-1">
              {activity.tasks.map((task) => (
                <li key={task.id} className="rounded-lg px-2 py-1.5 hover:bg-surface-hover">
                  <div className="flex items-start gap-2">
                    <div className="min-w-0 flex-1">
                      <p className="truncate font-mono text-note text-text-primary" title={task.command}>
                        {task.command}
                      </p>
                      <p className="mt-0.5 text-caption text-text-tertiary">
                        {backgroundSourceLabel(task.source)} · {BACKGROUND_STATUS_LABEL[task.status]}
                        {task.exit_code != null ? ` · 退出码 ${task.exit_code}` : ""}
                      </p>
                    </div>
                    {task.status === "running" && (
                      <button
                        type="button"
                        aria-label={`停止后台任务 ${task.command}`}
                        onClick={() => void stopTask(task.id)}
                        className="flex min-h-9 items-center gap-1 rounded-lg border border-border px-1.5 text-caption text-text-secondary hover:bg-surface"
                      >
                        <Square size={14} /> 停止
                      </button>
                    )}
                  </div>
                </li>
              ))}
            </ul>
          )}

          {activity.wakeups.length > 0 && (
            <ul className="mt-1 space-y-1 border-t border-border pt-1">
              {activity.wakeups.map((wakeup) => (
                <li key={wakeup.id} className="flex items-start gap-2 rounded-lg px-2 py-1.5">
                  <Timer size={14} className="mt-0.5 shrink-0 text-text-tertiary" />
                  <div className="min-w-0 flex-1">
                    <p className="truncate text-note text-text-primary" title={wakeup.prompt}>
                      {wakeup.prompt}
                    </p>
                    <p className="mt-0.5 text-caption text-text-tertiary">
                      {backgroundSourceLabel(wakeup.source)} · {wakeupCadence(wakeup)}
                      {wakeup.fire_count > 0 ? ` · 已触发 ${wakeup.fire_count} 次` : ""}
                    </p>
                  </div>
                  <button
                    type="button"
                    aria-label={`取消定时唤醒 ${wakeup.prompt}`}
                    onClick={() => void cancelWakeup(wakeup.id)}
                    className="flex min-h-9 items-center gap-1 rounded-lg border border-border px-1.5 text-caption text-text-secondary hover:bg-surface-hover"
                  >
                    <X size={14} /> 取消
                  </button>
                </li>
              ))}
            </ul>
          )}

          {error && (
            <p className="mt-1 px-2 py-1 text-caption text-status-danger">{error}</p>
          )}
        </div>
      )}
    </div>
  );
}
