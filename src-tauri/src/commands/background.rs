// SPDX-License-Identifier: Apache-2.0
//! CF-BG 阶段 3（R6）的界面后端：会话内后台任务 / 定时唤醒的只读列表与停止入口。
//!
//! 这里只做**按会话过滤**的查询与转调，进程、持久化、回收都在
//! `crate::agent::background_tasks`，因此界面看到的就是 agent 工具看到的同一份记录。
//! 不放宽任何权限：停止/取消与 agent 工具走同一个函数。

use serde::Serialize;
use tauri::State;

use crate::agent::background_tasks::{self, BackgroundTask, WakeupSchedule};
use crate::errors::AppError;
use crate::AppState;

#[derive(Debug, Serialize)]
pub struct SessionBackgroundActivity {
    pub tasks: Vec<BackgroundTask>,
    pub wakeups: Vec<WakeupSchedule>,
    pub commands: Vec<crate::util::process_tree::CommandActivity>,
}

/// 本会话的后台任务与待触发唤醒。界面只显示，不把唤醒回路写进消息流。
#[tauri::command]
pub async fn list_session_background_activity(
    state: State<'_, AppState>,
    session_id: String,
) -> Result<SessionBackgroundActivity, AppError> {
    let pool = state.db.read().await.clone();
    let tasks = background_tasks::list_session(&pool, &session_id)
        .await
        .map_err(AppError::Other)?;
    let mut wakeups = background_tasks::list_wakeups(&pool, &session_id)
        .await
        .map_err(AppError::Other)?;
    wakeups.retain(|wakeup| wakeup.is_pending());
    let commands = crate::util::process_tree::command_activity(&session_id);
    Ok(SessionBackgroundActivity { tasks, wakeups, commands })
}

#[tauri::command]
pub async fn stop_session_background_task(
    state: State<'_, AppState>,
    session_id: String,
    task_id: String,
) -> Result<(), AppError> {
    if crate::util::process_tree::stop_command(&session_id, &task_id) {
        return Ok(());
    }
    let pool = state.db.read().await.clone();
    background_tasks::stop(&pool, &session_id, &task_id)
        .await
        .map(|_| ())
        .map_err(AppError::Other)
}

#[tauri::command]
pub async fn cancel_session_wakeup(
    state: State<'_, AppState>,
    session_id: String,
    wakeup_id: String,
) -> Result<WakeupSchedule, AppError> {
    let pool = state.db.read().await.clone();
    background_tasks::cancel_wakeup(&pool, &session_id, &wakeup_id)
        .await
        .map_err(AppError::Other)
}
