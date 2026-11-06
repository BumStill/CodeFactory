// SPDX-License-Identifier: Apache-2.0
//! CF-BG 阶段 1 — 会话级后台命令（R1 后台命令与唤醒、R3 管理与上限、R4 与前台同权限）。
//!
//! 规格：`docs/specs/feature-specs/session-background-tasks.md`。
//!
//! 这一层刻意不依赖 Tauri：进程启动、监视、对账、上限、进程树回收都是纯 Rust，
//! 需要应用的装配（数据目录、唤醒入队）由调用方注入，因此 headless 测试可以覆盖
//! 真实进程与真实 SQLite，而不是 mock。

use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::time::Duration;

use crate::util::no_window::NoWindow;
use serde::{Deserialize, Serialize};
use sqlx::{Row, SqlitePool};

/// D6：每会话并发后台任务上限。
pub const MAX_RUNNING_PER_SESSION: i64 = 3;
/// D6：全局并发后台任务上限。
pub const MAX_RUNNING_GLOBAL: i64 = 8;
/// D6：单任务最长时长（3 小时）。
pub const MAX_TASK_SECS: u64 = 10_800;
/// 默认单任务时长。
pub const DEFAULT_TASK_SECS: u64 = 1_800;
/// 唤醒文本里输出尾部的上限（D2 / Payload Harness）。
pub const WAKE_OUTPUT_LIMIT: usize = 2_000;
/// `background_task_output` 返回的输出上限。
pub const OUTPUT_TOOL_LIMIT: usize = 20_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Running,
    Exited,
    Failed,
    TimedOut,
    Stopped,
    Lost,
}

impl TaskStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Exited => "exited",
            Self::Failed => "failed",
            Self::TimedOut => "timed_out",
            Self::Stopped => "stopped",
            Self::Lost => "lost",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "running" => Self::Running,
            "exited" => Self::Exited,
            "failed" => Self::Failed,
            "timed_out" => Self::TimedOut,
            "stopped" => Self::Stopped,
            "lost" => Self::Lost,
            _ => return None,
        })
    }

    pub fn is_terminal(self) -> bool {
        !matches!(self, Self::Running)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskSource {
    AgentDecided,
    UserRequested,
}

impl TaskSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AgentDecided => "agent_decided",
            Self::UserRequested => "user_requested",
        }
    }

    pub fn parse(value: &str) -> Self {
        match value {
            "user_requested" => Self::UserRequested,
            _ => Self::AgentDecided,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackgroundTask {
    pub id: String,
    pub session_id: String,
    pub root_turn_id: Option<String>,
    pub command: String,
    pub cwd: String,
    pub source: TaskSource,
    pub status: TaskStatus,
    pub exit_code: Option<i32>,
    pub pid: Option<u32>,
    pub process_started_at: Option<String>,
    pub output_path: String,
    pub created_at: i64,
    pub finished_at: Option<i64>,
    pub timeout_sec: u64,
    pub wake_state: String,
}

impl BackgroundTask {
    pub fn is_terminal(&self) -> bool {
        self.status.is_terminal()
    }
}

pub struct StartRequest {
    pub session_id: String,
    pub root_turn_id: Option<String>,
    pub cwd: PathBuf,
    pub output_dir: PathBuf,
    pub command: String,
    pub source: TaskSource,
    pub timeout_sec: Option<u64>,
}

#[derive(Debug, Clone)]
pub struct Started {
    pub id: String,
    pub pid: Option<u32>,
    pub output_path: String,
}

// ── store ────────────────────────────────────────────────────────────────────

pub async fn ensure_schema(pool: &SqlitePool) -> sqlx::Result<()> {
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS background_tasks (
            id TEXT PRIMARY KEY,
            session_id TEXT NOT NULL,
            root_turn_id TEXT,
            command TEXT NOT NULL,
            cwd TEXT NOT NULL,
            source TEXT NOT NULL,
            status TEXT NOT NULL,
            exit_code INTEGER,
            pid INTEGER,
            process_started_at TEXT,
            output_path TEXT NOT NULL,
            created_at INTEGER NOT NULL,
            finished_at INTEGER,
            timeout_sec INTEGER NOT NULL,
            wake_state TEXT NOT NULL DEFAULT 'none'
        )",
    )
    .execute(pool)
    .await?;
    sqlx::query(
        "CREATE INDEX IF NOT EXISTS idx_background_tasks_session ON background_tasks(session_id, status)",
    )
    .execute(pool)
    .await?;
    sqlx::query("CREATE INDEX IF NOT EXISTS idx_background_tasks_status ON background_tasks(status)")
        .execute(pool)
        .await?;
    sqlx::query("CREATE INDEX IF NOT EXISTS idx_background_tasks_wake ON background_tasks(wake_state)")
        .execute(pool)
        .await?;
    // R2：定时唤醒表同属本模块 schema，旧库启动时一并补齐（Compatibility）。
    ensure_wakeup_schema(pool).await?;
    Ok(())
}

fn row_to_task(row: &sqlx::sqlite::SqliteRow) -> BackgroundTask {
    let status: String = row.get("status");
    let source: String = row.get("source");
    let exit_code: Option<i64> = row.get("exit_code");
    let pid: Option<i64> = row.get("pid");
    let timeout_sec: i64 = row.get("timeout_sec");
    BackgroundTask {
        id: row.get("id"),
        session_id: row.get("session_id"),
        root_turn_id: row.get("root_turn_id"),
        command: row.get("command"),
        cwd: row.get("cwd"),
        source: TaskSource::parse(&source),
        status: TaskStatus::parse(&status).unwrap_or(TaskStatus::Lost),
        exit_code: exit_code.map(|code| code as i32),
        pid: pid.map(|pid| pid as u32),
        process_started_at: row.get("process_started_at"),
        output_path: row.get("output_path"),
        created_at: row.get("created_at"),
        finished_at: row.get("finished_at"),
        timeout_sec: timeout_sec.max(0) as u64,
        wake_state: row.get("wake_state"),
    }
}

pub async fn get(pool: &SqlitePool, id: &str) -> Result<Option<BackgroundTask>, String> {
    ensure_schema(pool).await.map_err(|error| error.to_string())?;
    let row = sqlx::query("SELECT * FROM background_tasks WHERE id = ?")
        .bind(id)
        .fetch_optional(pool)
        .await
        .map_err(|error| error.to_string())?;
    Ok(row.as_ref().map(row_to_task))
}

pub async fn list_session(
    pool: &SqlitePool,
    session_id: &str,
) -> Result<Vec<BackgroundTask>, String> {
    ensure_schema(pool).await.map_err(|error| error.to_string())?;
    let rows = sqlx::query(
        "SELECT * FROM background_tasks WHERE session_id = ? ORDER BY created_at DESC, id",
    )
    .bind(session_id)
    .fetch_all(pool)
    .await
    .map_err(|error| error.to_string())?;
    Ok(rows.iter().map(row_to_task).collect())
}

async fn running_rows(pool: &SqlitePool) -> Result<Vec<BackgroundTask>, String> {
    let rows = sqlx::query("SELECT * FROM background_tasks WHERE status = 'running' ORDER BY created_at")
        .fetch_all(pool)
        .await
        .map_err(|error| error.to_string())?;
    Ok(rows.iter().map(row_to_task).collect())
}

/// D6：上限判断。超限时返回一句可直接展示给用户的错误，不创建任何行。
async fn admit(pool: &SqlitePool, session_id: &str) -> Result<(), String> {
    let running = running_rows(pool).await?;
    let session_running = running
        .iter()
        .filter(|task| task.session_id == session_id)
        .count();
    if session_running as i64 >= MAX_RUNNING_PER_SESSION {
        return Err(format!(
            "本会话已有 {session_running} 个后台任务在运行，达到单会话上限 {MAX_RUNNING_PER_SESSION}。\
             先等其中一个结束（结束后会自动唤醒本会话），或用 background_task_stop 停掉不用的任务。"
        ));
    }
    if running.len() as i64 >= MAX_RUNNING_GLOBAL {
        return Err(format!(
            "当前共有 {} 个后台任务在运行，达到全局上限 {MAX_RUNNING_GLOBAL}。\
             请先结束其中一个后台任务再启动新的。",
            running.len()
        ));
    }
    Ok(())
}

pub fn validate_timeout(timeout_sec: Option<u64>) -> Result<u64, String> {
    let timeout = timeout_sec.unwrap_or(DEFAULT_TASK_SECS);
    if timeout == 0 {
        return Err("timeout_sec 必须大于 0。".to_string());
    }
    if timeout > MAX_TASK_SECS {
        return Err(format!(
            "timeout_sec={timeout} 超过单任务最长时长上限 {MAX_TASK_SECS} 秒（3 小时）。\
             需要更长时间的等待时，请用多个短任务接续，或让命令自己带截止时间。"
        ));
    }
    Ok(timeout)
}

async fn settle(
    pool: &SqlitePool,
    id: &str,
    status: TaskStatus,
    exit_code: Option<i32>,
    notify: bool,
) -> bool {
    let now = now_secs();
    let wake_state = if notify { "pending" } else { "none" };
    let result = sqlx::query(
        "UPDATE background_tasks
            SET status = ?, exit_code = ?, finished_at = ?, wake_state = ?
          WHERE id = ? AND status = 'running'",
    )
    .bind(status.as_str())
    .bind(exit_code.map(|code| code as i64))
    .bind(now)
    .bind(wake_state)
    .bind(id)
    .execute(pool)
    .await;
    matches!(result, Ok(done) if done.rows_affected() == 1)
}

// ── 输出与唤醒文本 ────────────────────────────────────────────────────────────

/// 读取输出尾部：保留最后 `limit` 字节，但不切坏多字节字符。
pub fn output_tail(path: &Path, limit: usize) -> String {
    let Ok(bytes) = std::fs::read(path) else {
        return String::new();
    };
    // Decode before truncating: a raw byte offset may split CP936 characters.
    let text = crate::util::command_env::decode_output(&bytes);
    let mut start = text.len().saturating_sub(limit);
    while !text.is_char_boundary(start) {
        start += 1;
    }
    let mut tail = text[start..].to_owned();
    #[cfg(windows)]
    {
        let shell = crate::util::command_env::shell_invocation("");
        tail.push_str(crate::util::command_env::shell_failure_hint(shell.program, &text));
    }
    tail
}

fn status_sentence(task: &BackgroundTask) -> String {
    match task.status {
        TaskStatus::Exited => match task.exit_code {
            Some(code) => format!("退出码 {code}"),
            None => "退出码未知".to_string(),
        },
        TaskStatus::Failed => match task.exit_code {
            Some(code) => format!("失败，退出码 {code}"),
            None => "失败（退出码未知）".to_string(),
        },
        TaskStatus::TimedOut => format!("超时（{} 秒上限已到，进程树已结束）", task.timeout_sec),
        TaskStatus::Stopped => "被停止".to_string(),
        TaskStatus::Lost => "无法确认结果（重启后找不到对应进程）".to_string(),
        TaskStatus::Running => "仍在运行".to_string(),
    }
}

/// D2：唤醒文本的固定形状——任务 id + 结局 + 输出尾部 + 输出文件。
pub fn wake_text(task: &BackgroundTask) -> String {
    let tail = output_tail(Path::new(&task.output_path), WAKE_OUTPUT_LIMIT);
    let mut text = format!(
        "后台任务 {} 已结束：{}。\n命令：{}\ncwd：{}\n",
        task.id,
        status_sentence(task),
        task.command,
        task.cwd
    );
    if tail.trim().is_empty() {
        text.push_str("输出尾部：（无输出）\n");
    } else {
        text.push_str("输出尾部：\n");
        text.push_str(tail.trim_end());
        text.push('\n');
    }
    text.push_str(&format!("完整输出：{}", task.output_path));
    text
}

// ── 平台进程操作 ──────────────────────────────────────────────────────────────

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(unix)]
fn shell_invocation(command: &str) -> (String, Vec<String>) {
    let program = if Path::new("/bin/sh").exists() {
        "/bin/sh".to_string()
    } else {
        "sh".to_string()
    };
    (program, vec!["-c".to_string(), command.to_string()])
}

#[cfg(windows)]
fn shell_invocation(command: &str) -> (String, Vec<String>) {
    let shell = crate::util::command_env::shell_invocation(command);
    (shell.program.to_string(), shell.args)
}

/// 进程启动时间的可比对标记；取不到时返回 None（对账必须落到 lost，不猜 running）。
#[cfg(target_os = "macos")]
pub fn process_start_marker(pid: u32) -> Option<String> {
    let output = std::process::Command::new("/bin/ps")
        .no_window()
        .args(["-o", "lstart=", "-p", &pid.to_string()])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!text.is_empty()).then_some(text)
}

#[cfg(all(unix, not(target_os = "macos")))]
pub fn process_start_marker(pid: u32) -> Option<String> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let tail = &stat[stat.rfind(')')? + 1..];
    // 字段 3 = state，所以字段 22（starttime）在切片后的索引 19。
    tail.split_whitespace().nth(19).map(|value| value.to_string())
}

#[cfg(windows)]
fn windows_process_identity(pid: u32) -> Option<(String, bool)> {
    use windows_sys::Win32::{
        Foundation::{CloseHandle, FILETIME},
        System::Threading::{GetExitCodeProcess, GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION},
    };
    // SAFETY: 只以查询权限打开进程；所有输出指针均为有效局部变量，句柄在返回前关闭。
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() {
            return None;
        }
        let mut created: FILETIME = std::mem::zeroed();
        let mut exited: FILETIME = std::mem::zeroed();
        let mut kernel: FILETIME = std::mem::zeroed();
        let mut user: FILETIME = std::mem::zeroed();
        let mut code = 0;
        let times_ok = GetProcessTimes(handle, &mut created, &mut exited, &mut kernel, &mut user) != 0;
        let exit_ok = GetExitCodeProcess(handle, &mut code) != 0;
        CloseHandle(handle);
        if !times_ok || !exit_ok {
            return None;
        }
        let ticks = ((created.dwHighDateTime as u64) << 32) | created.dwLowDateTime as u64;
        // .NET DateTime ticks（原持久化格式）与 FILETIME 的纪元差，保留旧记录兼容性。
        Some(((ticks + 504_911_232_000_000_000).to_string(), code == 259))
    }
}

#[cfg(windows)]
pub fn process_start_marker(pid: u32) -> Option<String> {
    windows_process_identity(pid).and_then(|(marker, live)| live.then_some(marker))
}

#[cfg(unix)]
fn process_alive(pid: u32) -> bool {
    // SAFETY: kill(pid, 0) 只做存在性与权限探测，不发送信号。
    unsafe { libc::kill(pid as i32, 0) == 0 }
}

#[cfg(windows)]
fn process_alive(pid: u32) -> bool {
    windows_process_identity(pid).is_some_and(|(_, live)| live)
}

#[cfg(unix)]
fn signal_process_group(pid: u32, signal: i32) {
    // 负 pid = 整个进程组（启动时 process_group(0) 使 pgid == pid）。
    // SAFETY: 只向本任务创建的进程组发信号。
    unsafe {
        libc::kill(-(pid as i32), signal);
    }
}

#[cfg(unix)]
fn process_group_alive(pid: u32) -> bool {
    // SAFETY: 与上面同理，只是存在性探测。
    unsafe { libc::kill(-(pid as i32), 0) == 0 }
}

#[cfg(windows)]
fn signal_process_group(pid: u32, _signal: i32) {
    let _ = std::process::Command::new("taskkill")
        .no_window()
        .args(["/PID", &pid.to_string(), "/T", "/F"])
        .output();
}

#[cfg(windows)]
fn process_group_alive(pid: u32) -> bool {
    process_alive(pid)
}

/// D5：先 SIGTERM，再有界等待，仍在则 SIGKILL；Windows 用 `taskkill /T /F`。
pub async fn terminate_tree(pid: u32) {
    #[cfg(unix)]
    {
        signal_process_group(pid, libc::SIGTERM);
        for _ in 0..30 {
            if !process_group_alive(pid) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        signal_process_group(pid, libc::SIGKILL);
        for _ in 0..10 {
            if !process_group_alive(pid) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
    #[cfg(windows)]
    {
        signal_process_group(pid, 0);
        for _ in 0..20 {
            if !process_group_alive(pid) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
}

// ── 启动 / 监视 ───────────────────────────────────────────────────────────────

/// 步骤 1：分类与上限。任何一步失败都不落库、不 spawn（R4 + D6）。
pub fn preflight(command: &str, timeout_sec: Option<u64>) -> Result<u64, String> {
    if let crate::tools::shell_policy::ShellCommandPolicy::Deny { reason } =
        crate::tools::shell_policy::classify_command(command)
    {
        return Err(format!(
            "后台命令被拒绝（与前台 bash 同一份分类规则）：{reason}"
        ));
    }
    validate_timeout(timeout_sec)
}

pub async fn start(pool: &SqlitePool, request: StartRequest) -> Result<Started, String> {
    ensure_schema(pool).await.map_err(|error| error.to_string())?;
    let timeout_sec = preflight(&request.command, request.timeout_sec)?;
    admit(pool, &request.session_id).await?;

    std::fs::create_dir_all(&request.output_dir).map_err(|error| error.to_string())?;
    let id = format!("bg-{}", uuid::Uuid::new_v4().simple());
    let output_path = request.output_dir.join(format!("{id}.log"));
    let output_file = std::fs::File::create(&output_path).map_err(|error| error.to_string())?;
    let cwd_text = request.cwd.to_string_lossy().to_string();
    let created_at = now_secs();

    // 先落库再 spawn：进程存在时一定有记录。
    sqlx::query(
        "INSERT INTO background_tasks
            (id, session_id, root_turn_id, command, cwd, source, status, exit_code, pid,
             process_started_at, output_path, created_at, finished_at, timeout_sec, wake_state)
         VALUES (?, ?, ?, ?, ?, ?, 'running', NULL, NULL, NULL, ?, ?, NULL, ?, 'none')",
    )
    .bind(&id)
    .bind(&request.session_id)
    .bind(&request.root_turn_id)
    .bind(&request.command)
    .bind(&cwd_text)
    .bind(request.source.as_str())
    .bind(output_path.to_string_lossy().to_string())
    .bind(created_at)
    .bind(timeout_sec as i64)
    .execute(pool)
    .await
    .map_err(|error| error.to_string())?;

    let (program, args) = shell_invocation(&request.command);
    // M224/R3：后台任务与前台 bash 共用同一个沙箱入口，不允许一个在沙箱内、
    // 一个在沙箱外。沙箱不可用时退回审批路径（spawn 前已被网关放行）。
    let (program, args, _sandbox) =
        crate::agent::trusted_sandbox::wrap_command(&request.cwd, None, &program, &args);
    let mut command = tokio::process::Command::new(&program).no_window();
    command
        .args(&args)
        .current_dir(&request.cwd)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::from(
            output_file.try_clone().map_err(|error| error.to_string())?,
        ))
        .stderr(std::process::Stdio::from(output_file))
        .kill_on_drop(false);
    crate::util::command_env::apply_noninteractive(&mut command);
    crate::util::command_env::apply_developer_path(&mut command);
    // D5：独立进程组，停止/超时才能杀整棵树。
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.as_std_mut().process_group(0);
    }

    let child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            let _ = settle(pool, &id, TaskStatus::Failed, None, true).await;
            return Err(format!("后台命令启动失败：{error}"));
        }
    };
    let pid = child.id();
    #[cfg(windows)]
    let job = match child.raw_handle()
        .ok_or_else(|| std::io::Error::other("missing child process handle"))
        .and_then(crate::util::command_activity::attach_job) {
        Ok(job) => job,
        Err(error) => {
            let mut child = child;
            let _ = child.kill().await;
            settle(pool, &id, TaskStatus::Failed, None, true).await;
            return Err(format!("后台命令无法加入 Job Object，已停止：{error}"));
        }
    };
    let marker = pid.and_then(process_start_marker);
    if let Some(pid) = pid {
        let _ = sqlx::query("UPDATE background_tasks SET pid = ?, process_started_at = ? WHERE id = ?")
            .bind(pid as i64)
            .bind(marker)
            .bind(&id)
            .execute(pool)
            .await;
    }

    tokio::spawn(monitor_child(
        pool.clone(),
        id.clone(),
        child,
        pid,
        Duration::from_secs(timeout_sec),
        output_path.clone(),
        #[cfg(windows)]
        job,
    ));

    Ok(Started {
        id,
        pid,
        output_path: output_path.to_string_lossy().to_string(),
    })
}

async fn monitor_child(
    pool: SqlitePool,
    id: String,
    mut child: tokio::process::Child,
    pid: Option<u32>,
    timeout: Duration,
    output_path: PathBuf,
    #[cfg(windows)] job: crate::util::command_activity::JobGuard,
) {
    let deadline = tokio::time::Instant::now() + timeout;
    let mut tick = tokio::time::interval(Duration::from_millis(200));
    loop {
        tokio::select! {
            result = child.wait() => {
                let code = result.ok().and_then(|status| status.code());
                let tail = output_tail(&output_path, 4096);
                if crate::util::command_env::waiting_for_input(&tail) {
                    append_input_notice(&output_path);
                    settle(&pool, &id, TaskStatus::Failed, code, true).await;
                } else {
                    settle(&pool, &id, if code == Some(0) { TaskStatus::Exited } else { TaskStatus::Failed }, code, true).await;
                }
                return;
            }
            _ = tick.tick() => {
                let stopped = get(&pool, &id).await.ok().flatten().is_some_and(|task| task.status == TaskStatus::Stopped);
                let input = crate::util::command_env::waiting_for_input(&output_tail(&output_path, 4096));
                let timed_out = tokio::time::Instant::now() >= deadline;
                if !stopped && !input && !timed_out { continue; }
                #[cfg(windows)]
                drop(job);
                if let Some(pid) = pid { terminate_tree(pid).await; }
                let _ = tokio::time::timeout(Duration::from_secs(1), child.wait()).await;
                if input { append_input_notice(&output_path); }
                let status = if stopped { TaskStatus::Stopped } else if input { TaskStatus::Failed } else { TaskStatus::TimedOut };
                settle(&pool, &id, status, None, true).await;
                return;
            }
        }
    }
}

fn append_input_notice(path: &Path) {
    use std::io::Write;
    if let Ok(mut log) = std::fs::OpenOptions::new().append(true).open(path) {
        let _ = writeln!(log, "\n{}", crate::util::command_env::INPUT_REQUIRED_MESSAGE);
    }
}

// ── 对账（重启后） ────────────────────────────────────────────────────────────

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ReconcileReport {
    pub resumed: usize,
    pub finished: usize,
    pub lost: usize,
}

enum Verdict {
    Live,
    Dead,
    Unknown,
}

fn verdict_for(task: &BackgroundTask) -> Verdict {
    let (Some(pid), Some(recorded)) = (task.pid, task.process_started_at.as_deref()) else {
        return Verdict::Unknown;
    };
    match process_start_marker(pid) {
        Some(current) if current == recorded => Verdict::Live,
        Some(_) => Verdict::Unknown, // pid 被复用：不是同一个进程
        None => {
            if process_alive(pid) {
                Verdict::Unknown
            } else {
                Verdict::Dead
            }
        }
    }
}

/// D4：启动对账。幂等——重复执行只做"running → 终态"的单向转换。
pub async fn reconcile(pool: &SqlitePool) -> Result<ReconcileReport, String> {
    ensure_schema(pool).await.map_err(|error| error.to_string())?;
    let mut report = ReconcileReport::default();
    for task in running_rows(pool).await? {
        match verdict_for(&task) {
            Verdict::Live => {
                report.resumed += 1;
                tokio::spawn(watch_orphan(pool.clone(), task));
            }
            Verdict::Dead => {
                // M168/R1：输出文件不是退出收据，重启后已死进程按 lost 结算。
                if settle(pool, &task.id, TaskStatus::Lost, None, true).await {
                    report.lost += 1;
                }
            }
            Verdict::Unknown => {
                if settle(pool, &task.id, TaskStatus::Lost, None, true).await {
                    report.lost += 1;
                }
            }
        }
    }
    Ok(report)
}

/// 已确认仍在运行的孤儿：轮询到它消失或超时，然后补写终态并发唤醒。
async fn watch_orphan(pool: SqlitePool, task: BackgroundTask) {
    let Some(pid) = task.pid else {
        settle(&pool, &task.id, TaskStatus::Lost, None, true).await;
        return;
    };
    let deadline = tokio::time::Instant::now()
        + Duration::from_secs(task.timeout_sec.max(1));
    loop {
        if !process_alive(pid) {
            // After restart we can observe disappearance, not an exit receipt.
            // A log file may contain partial output from a failed/lost process.
            settle(&pool, &task.id, TaskStatus::Lost, None, true).await;
            return;
        }
        if tokio::time::Instant::now() >= deadline {
            terminate_tree(pid).await;
            settle(&pool, &task.id, TaskStatus::TimedOut, None, true).await;
            return;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

// ── 停止 / 会话回收 ──────────────────────────────────────────────────────────

/// R3：按 task_id 停止；杀整个进程树，终态 `stopped`。
pub async fn stop(pool: &SqlitePool, session_id: &str, id: &str) -> Result<BackgroundTask, String> {
    let task = get(pool, id)
        .await?
        .ok_or_else(|| format!("后台任务 {id} 不存在。"))?;
    if task.session_id != session_id {
        return Err(format!("后台任务 {id} 不属于本会话，不能从本会话停止。"));
    }
    if task.status.is_terminal() {
        return Ok(task);
    }
    // 先认领终态再杀进程树：否则监视任务会先把信号退出记成 failed，
    // 用户点"停止"的任务就永远显示成失败（真机上是可见的错误状态）。
    if !settle(pool, &task.id, TaskStatus::Stopped, None, true).await {
        return get(pool, id)
            .await?
            .ok_or_else(|| format!("后台任务 {id} 在停止后消失。"));
    }
    if let Some(pid) = task.pid {
        terminate_tree(pid).await;
    }
    get(pool, id)
        .await?
        .ok_or_else(|| format!("后台任务 {id} 在停止后消失。"))
}

/// R3 / R2：会话被停止/结案时回收全部后台任务与定时唤醒，不再发唤醒。
pub async fn reclaim_session(pool: &SqlitePool, session_id: &str) -> usize {
    if ensure_schema(pool).await.is_err() {
        return 0;
    }
    let mut reclaimed = 0;
    for task in running_rows(pool).await.unwrap_or_default() {
        if task.session_id != session_id {
            continue;
        }
        // 同上：先认领终态，再杀进程树。
        if !settle(pool, &task.id, TaskStatus::Stopped, None, false).await {
            continue;
        }
        if let Some(pid) = task.pid {
            terminate_tree(pid).await;
        }
        reclaimed += 1;
    }
    // 定时唤醒与后台任务同一回收入口：结案后不该再被叫醒。
    cancel_session_wakeups(pool, session_id).await;
    reclaimed
}

// ── 唤醒投递 ─────────────────────────────────────────────────────────────────

pub type WakeFuture = Pin<Box<dyn std::future::Future<Output = bool> + Send>>;

/// 唤醒入队通道。生产实现走与用户输入相同的会话入队；测试用记录型实现。
pub trait WakeSink: Send + Sync {
    fn enqueue(&self, session_id: String, text: String) -> WakeFuture;
    fn enqueue_detailed(&self, session_id: String, text: String) -> Pin<Box<dyn std::future::Future<Output = Result<bool, String>> + Send>> {
        let future = self.enqueue(session_id, text);
        Box::pin(async move { if future.await { Ok(true) } else { Err("唤醒通道拒绝准入".into()) } })
    }
}

/// D2：把待投递的唤醒交给会话入队，投递成功才标记 `enqueued`，因此重启后不会重复补发。
/// M168：统一持久投递边界。单会话准入阻塞不能永久挂起整个投递器。
async fn deliver_wake(sink: &dyn WakeSink, session: String, text: String, due_at: i64) -> bool {
    if now_secs().saturating_sub(due_at) >= 300 {
        tracing::warn!(session, pending_seconds = now_secs().saturating_sub(due_at),
            "wake pending over five minutes; retrying immediately");
    }
    match tokio::time::timeout(Duration::from_secs(5), sink.enqueue(session.clone(), text)).await {
        Ok(delivered) => delivered,
        Err(_) => {
            tracing::warn!(session, "wake admission timed out; durable wake remains pending for retry");
            false
        }
    }
}

pub async fn drain_pending_wakes(pool: &SqlitePool, sink: &dyn WakeSink) -> usize {
    if ensure_schema(pool).await.is_err() {
        return 0;
    }
    let rows = sqlx::query(
        "SELECT * FROM background_tasks WHERE wake_state = 'pending' ORDER BY finished_at LIMIT 32",
    )
    .fetch_all(pool)
    .await
    .unwrap_or_default();
    use futures_util::{stream::FuturesUnordered, StreamExt};
    let mut pending = FuturesUnordered::new();
    for row in rows {
        let task = row_to_task(&row);
        pending.push(async move {
            match settle_objectiveless_wake(pool, &task.session_id, now_secs()).await {
                Ok(true) => return 0,
                Ok(false) => {},
                Err(error) => {
                    defer_objective_probe(pool, &task.session_id, now_secs(), &error.to_string()).await;
                    return 0;
                }
            }
            let text = wake_text(&task);
            if !deliver_wake(sink, task.session_id.clone(), text, task.finished_at.unwrap_or(task.created_at)).await { return 0; }
            sqlx::query("UPDATE background_tasks SET wake_state='enqueued' WHERE id=? AND wake_state='pending'")
                .bind(&task.id).execute(pool).await.map_or(0, |done| done.rows_affected() as usize)
        });
    }
    let mut delivered = 0;
    while let Some(count) = pending.next().await { delivered += count; }
    delivered
}

// ── R2 定时唤醒 ──────────────────────────────────────────────────────────────

/// R2：唤醒间隔下限（60 秒）。
pub const MIN_WAKEUP_SECS: u64 = 60;
/// R2：唤醒间隔上限（6 小时）。
pub const MAX_WAKEUP_SECS: u64 = 6 * 3_600;
/// R2：每会话待触发唤醒上限。
pub const MAX_WAKEUPS_PER_SESSION: i64 = 8;
/// 唤醒提示文本长度上限（Payload Harness）。
pub const WAKEUP_PROMPT_LIMIT: usize = 4_000;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WakeupSchedule {
    pub id: String,
    pub session_id: String,
    pub root_turn_id: Option<String>,
    pub prompt: String,
    pub source: TaskSource,
    pub after_sec: Option<u64>,
    pub every_sec: Option<u64>,
    pub next_at: i64,
    pub status: String,
    pub fire_count: i64,
    pub created_at: i64,
    pub last_fired_at: Option<i64>,
}

impl WakeupSchedule {
    pub fn is_pending(&self) -> bool {
        self.status == "pending"
    }

    /// 唤醒节奏的一行说明（列表与唤醒文本共用）。
    pub fn cadence(&self) -> String {
        match self.every_sec {
            Some(every) => format!("每 {every} 秒一次"),
            None => format!("一次性（{} 秒后）", self.after_sec.unwrap_or(MIN_WAKEUP_SECS)),
        }
    }
}

pub struct WakeupRequest {
    pub session_id: String,
    pub root_turn_id: Option<String>,
    pub prompt: String,
    pub source: TaskSource,
    pub after_sec: Option<u64>,
    pub every_sec: Option<u64>,
}

/// R2：`after_sec` / `every_sec` 二选一，且都必须落在 [60s, 6h]。
pub fn validate_wakeup_delay(
    after_sec: Option<u64>,
    every_sec: Option<u64>,
) -> Result<(Option<u64>, Option<u64>), String> {
    // M149 R1：模型习惯一次把两个字段都写上，不用的那个填 0 表示「没有」。
    // 0 与缺失、null 同义；否则一次性的等待会被判成二选一冲突，本回合就没有唤醒源。
    let after_sec = after_sec.filter(|value| *value > 0);
    let every_sec = every_sec.filter(|value| *value > 0);
    if after_sec.is_some() && every_sec.is_some() {
        return Err(
            "after_sec 与 every_sec 只能二选一：要一次性唤醒用 after_sec，要周期性唤醒用 every_sec；\
             不想用的那个字段省略或传 0/null。例：\
             {\"prompt\":\"检查 PR #123 的 CI 是否全绿\",\"after_sec\":180}"
                .to_string(),
        );
    }
    let value = match (after_sec, every_sec) {
        (Some(value), None) => value,
        (None, Some(value)) => value,
        // 上面已拒绝两者同时给出，这里保持穷尽。
        (Some(_), Some(_)) => unreachable!("after_sec 与 every_sec 已在前面互斥校验"),
        (None, None) => {
            return Err(format!(
                "必须给出唤醒时间：after_sec（{MIN_WAKEUP_SECS} 秒后唤醒一次）或 every_sec\
                 （每 N 秒唤醒一次），范围 {MIN_WAKEUP_SECS}–{MAX_WAKEUP_SECS} 秒。例：\
                 {{\"prompt\":\"检查 PR #123 的 CI 是否全绿\",\"after_sec\":180}}"
            ))
        }
    };
    if value < MIN_WAKEUP_SECS {
        return Err(format!(
            "唤醒间隔 {value} 秒低于下限 {MIN_WAKEUP_SECS} 秒。请用 ≥{MIN_WAKEUP_SECS} 秒的间隔\
             （上限 {MAX_WAKEUP_SECS} 秒 / 6 小时）；需要更早看到结果时，用 background_task_start 跑命令，\
             命令结束会立刻唤醒本会话。"
        ));
    }
    if value > MAX_WAKEUP_SECS {
        return Err(format!(
            "唤醒间隔 {value} 秒超过上限 {MAX_WAKEUP_SECS} 秒（6 小时）。请改用更短的间隔多次检查，\
             或挂一个带截止时间的后台命令。"
        ));
    }
    Ok((after_sec, every_sec))
}

fn row_to_wakeup(row: &sqlx::sqlite::SqliteRow) -> WakeupSchedule {
    let source: String = row.get("source");
    let after_sec: Option<i64> = row.get("after_sec");
    let every_sec: Option<i64> = row.get("every_sec");
    WakeupSchedule {
        id: row.get("id"),
        session_id: row.get("session_id"),
        root_turn_id: row.get("root_turn_id"),
        prompt: row.get("prompt"),
        source: TaskSource::parse(&source),
        after_sec: after_sec.map(|value| value.max(0) as u64),
        every_sec: every_sec.map(|value| value.max(0) as u64),
        next_at: row.get("next_at"),
        status: row.get("status"),
        fire_count: row.get("fire_count"),
        created_at: row.get("created_at"),
        last_fired_at: row.get("last_fired_at"),
    }
}

/// M186: the same schema guard, callable from the settlement path that cancels
/// wakeups inside its own transaction, and from tests that build a restart
/// fixture.
pub(crate) async fn ensure_wake_schema(pool: &SqlitePool) -> sqlx::Result<()> {
    ensure_wakeup_schema(pool).await
}

async fn ensure_wakeup_schema(pool: &SqlitePool) -> sqlx::Result<()> {
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS wakeup_schedules (
            id TEXT PRIMARY KEY,
            session_id TEXT NOT NULL,
            root_turn_id TEXT,
            prompt TEXT NOT NULL,
            source TEXT NOT NULL,
            after_sec INTEGER,
            every_sec INTEGER,
            next_at INTEGER NOT NULL,
            status TEXT NOT NULL,
            fire_count INTEGER NOT NULL DEFAULT 0,
            created_at INTEGER NOT NULL,
            last_fired_at INTEGER
        )",
    )
    .execute(pool)
    .await?;
    sqlx::query(
        "CREATE INDEX IF NOT EXISTS idx_wakeup_schedules_due ON wakeup_schedules(status, next_at)",
    )
    .execute(pool)
    .await?;
    sqlx::query(
        "CREATE INDEX IF NOT EXISTS idx_wakeup_schedules_session ON wakeup_schedules(session_id, status)",
    )
    .execute(pool)
    .await?;
    sqlx::query("CREATE TABLE IF NOT EXISTS wakeup_alerts(session_id TEXT PRIMARY KEY,first_seen_at INTEGER NOT NULL,last_retry_at INTEGER NOT NULL,delay_secs INTEGER NOT NULL,retry_count INTEGER NOT NULL)")
        .execute(pool).await?;
    Ok(())
}

/// 系统模型等待不受用户工具的 60 秒/6 小时输入范围约束；绝对恢复时间直接落盘。
///
/// M210/R5：同一次等待续期时沿用仍在 pending 的那一行（避免同一等待出现两个唤醒）；
/// 唤醒已经触发过、模型仍不可用时，重挂**新 id** 的唤醒，让会话保持
/// `waiting_wake` 而不是被孤儿清理或升级成提问。
pub(crate) async fn persist_model_wakeup(
    pool: &SqlitePool, session: &str, root: &str, at_ms: i64, notice: &str,
) -> sqlx::Result<String> {
    ensure_wakeup_schema(pool).await?;
    super::thin_state::ensure(pool).await?;
    let base = format!("model-availability-{session}");
    let mut tx = crate::storage::db::begin_write(pool).await?;
    let ended: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM thin_session_state WHERE session_id=? AND state='ended'")
        .bind(session).fetch_one(&mut *tx).await?;
    if ended > 0 { return Ok(base); }
    let pending: Option<String> = sqlx::query_scalar(
        "SELECT id FROM wakeup_schedules
          WHERE session_id=? AND status='pending' AND id LIKE 'model-availability-%'
          ORDER BY next_at ASC, created_at ASC LIMIT 1",
    )
    .bind(session)
    .fetch_optional(&mut *tx)
    .await?;
    let id = match pending {
        Some(id) => id,
        None => {
            let seen: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM wakeup_schedules WHERE session_id=? AND id LIKE 'model-availability-%'",
            )
            .bind(session)
            .fetch_one(&mut *tx)
            .await?;
            if seen == 0 { base } else { format!("{base}-re{seen}") }
        }
    };
    sqlx::query(
        "INSERT INTO wakeup_schedules(id,session_id,root_turn_id,prompt,source,next_at,status,created_at)
         VALUES (?,?,?,?,'agent_decided',?,'pending',?)
         ON CONFLICT(id) DO UPDATE SET root_turn_id=excluded.root_turn_id,
           prompt=excluded.prompt,next_at=excluded.next_at,status='pending',last_fired_at=NULL",
    )
    .bind(&id)
    .bind(session).bind(root)
    .bind(format!("{notice}。系统自动重试/自愈：模型可用时间已到，重新检查凭据、使用时段和用户额度硬上限，继续原任务。"))
    .bind(at_ms.saturating_add(999).div_euclid(1000)).bind(now_secs())
    .execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(id)
}

/// R2：创建唤醒。时限、提示文本、上限都在落库前校验，因此被拒时不留任何行。
pub async fn schedule_wakeup_at(
    pool: &SqlitePool,
    request: WakeupRequest,
    now: i64,
) -> Result<WakeupSchedule, String> {
    ensure_wakeup_schema(pool)
        .await
        .map_err(|error| error.to_string())?;
    let prompt = request.prompt.trim().to_string();
    if prompt.is_empty() {
        return Err(
            "prompt 不能为空：唤醒时要有一句话告诉本会话醒来做什么（例如「看一下 PR 的 CI 状态」）。"
                .to_string(),
        );
    }
    if prompt.chars().count() > WAKEUP_PROMPT_LIMIT {
        return Err(format!(
            "prompt 过长（{} 字符，上限 {WAKEUP_PROMPT_LIMIT} 字符）。请改成一句可执行的提示。",
            prompt.chars().count()
        ));
    }
    let (after_sec, every_sec) = validate_wakeup_delay(request.after_sec, request.every_sec)?;

    let pending: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM wakeup_schedules WHERE session_id = ? AND status = 'pending'",
    )
    .bind(&request.session_id)
    .fetch_one(pool)
    .await
    .map_err(|error| error.to_string())?;
    if pending >= MAX_WAKEUPS_PER_SESSION {
        return Err(format!(
            "本会话已有 {pending} 条定时唤醒，达到每会话上限 {MAX_WAKEUPS_PER_SESSION}。\
             先用 wakeup_cancel 取消不再需要的唤醒，或等它们触发后再创建。"
        ));
    }

    let delay = every_sec.or(after_sec).unwrap_or(MIN_WAKEUP_SECS);
    let id = format!("wk-{}", uuid::Uuid::new_v4().simple());
    let next_at = now + delay as i64;
    sqlx::query(
        "INSERT INTO wakeup_schedules
            (id, session_id, root_turn_id, prompt, source, after_sec, every_sec, next_at,
             status, fire_count, created_at, last_fired_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, 'pending', 0, ?, NULL)",
    )
    .bind(&id)
    .bind(&request.session_id)
    .bind(&request.root_turn_id)
    .bind(&prompt)
    .bind(request.source.as_str())
    .bind(after_sec.map(|value| value as i64))
    .bind(every_sec.map(|value| value as i64))
    .bind(next_at)
    .bind(now)
    .execute(pool)
    .await
    .map_err(|error| error.to_string())?;
    tracing::info!(
        wakeup = %id,
        session = %request.session_id,
        source = request.source.as_str(),
        interval = delay,
        repeating = every_sec.is_some(),
        "wakeup scheduled"
    );

    Ok(WakeupSchedule {
        id,
        session_id: request.session_id,
        root_turn_id: request.root_turn_id,
        prompt,
        source: request.source,
        after_sec,
        every_sec,
        next_at,
        status: "pending".to_string(),
        fire_count: 0,
        created_at: now,
        last_fired_at: None,
    })
}

pub async fn schedule_wakeup(
    pool: &SqlitePool,
    request: WakeupRequest,
) -> Result<WakeupSchedule, String> {
    schedule_wakeup_at(pool, request, now_secs()).await
}

pub async fn list_wakeups(
    pool: &SqlitePool,
    session_id: &str,
) -> Result<Vec<WakeupSchedule>, String> {
    ensure_wakeup_schema(pool)
        .await
        .map_err(|error| error.to_string())?;
    let rows = sqlx::query(
        "SELECT * FROM wakeup_schedules WHERE session_id = ? ORDER BY created_at DESC, id",
    )
    .bind(session_id)
    .fetch_all(pool)
    .await
    .map_err(|error| error.to_string())?;
    Ok(rows.iter().map(row_to_wakeup).collect())
}

pub async fn get_wakeup(
    pool: &SqlitePool,
    id: &str,
) -> Result<Option<WakeupSchedule>, String> {
    ensure_wakeup_schema(pool)
        .await
        .map_err(|error| error.to_string())?;
    let row = sqlx::query("SELECT * FROM wakeup_schedules WHERE id = ?")
        .bind(id)
        .fetch_optional(pool)
        .await
        .map_err(|error| error.to_string())?;
    Ok(row.as_ref().map(row_to_wakeup))
}

/// R2：取消一条唤醒。幂等——已取消/已触发的行原样返回，不报错。
pub async fn cancel_wakeup(
    pool: &SqlitePool,
    session_id: &str,
    id: &str,
) -> Result<WakeupSchedule, String> {
    let wakeup = get_wakeup(pool, id)
        .await?
        .ok_or_else(|| format!("定时唤醒 {id} 不存在。"))?;
    if wakeup.session_id != session_id {
        return Err(format!("定时唤醒 {id} 不属于本会话，不能从本会话取消。"));
    }
    if !wakeup.is_pending() {
        return Ok(wakeup);
    }
    let mut tx = crate::storage::db::begin_write(pool).await.map_err(|error| error.to_string())?;
    sqlx::query("UPDATE wakeup_schedules SET status = 'cancelled' WHERE id = ? AND status = 'pending'")
        .bind(id).execute(&mut *tx).await.map_err(|error| error.to_string())?;
    preserve_cancelled_wait(&mut tx, session_id).await.map_err(|error| error.to_string())?;
    tx.commit().await.map_err(|error| error.to_string())?;
    tracing::info!(wakeup = %id, session = %session_id, "wakeup cancelled");
    get_wakeup(pool, id)
        .await?
        .ok_or_else(|| format!("定时唤醒 {id} 在取消后消失。"))
}

/// M216/R2：取消与等待投影保护共用同一写事务；历史/极简库没有投影时不改语义。
async fn preserve_cancelled_wait(tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>, session: &str) -> sqlx::Result<()> {
    let exists: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='thin_session_state'").fetch_one(&mut **tx).await?;
    if exists == 0 { return Ok(()); }
    let waiting: Option<String> = sqlx::query_scalar("SELECT waiting_for FROM thin_session_state WHERE session_id=? AND state='waiting_wake'")
        .bind(session).fetch_optional(&mut **tx).await?.flatten();
    let Some(what) = waiting else { return Ok(()); };
    let pending: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM wakeup_schedules WHERE session_id=? AND status='pending'").bind(session).fetch_one(&mut **tx).await?;
    if pending > 0 { return Ok(()); }
    let id = format!("thin-source-repair-{}", uuid::Uuid::new_v4());
    sqlx::query("INSERT INTO wakeup_schedules(id,session_id,prompt,source,next_at,status,created_at) VALUES (?,?,?,'agent_decided',?,'pending',?)")
        .bind(&id).bind(session).bind(format!("系统自动重试/自愈：继续处理{what}，核对当前事实并接续原任务。"))
        .bind(now_secs()).bind(now_secs()).execute(&mut **tx).await?;
    sqlx::query("UPDATE thin_session_state SET wake_source=?,updated_at=? WHERE session_id=? AND state='waiting_wake'")
        .bind(format!("pending wakeup：{id}")).bind(now_secs()*1000).bind(session).execute(&mut **tx).await?;
    Ok(())
}

/// M241：只有确定已无任务才结束会话。查询失败回滚，由调用方退避重试。
/// 保留 M209/M228 已存在的真实用户任务与模型恢复例外，不赋予新授权。
pub(crate) async fn settle_objectiveless_wake(pool: &SqlitePool, session: &str, now: i64) -> anyhow::Result<bool> {
    super::thin_state::ensure(pool).await?;
    ensure_wakeup_schema(pool).await?;
    super::workspace_recovery_budget::ensure(pool).await?;
    let retry_at: Option<i64> = sqlx::query_scalar("SELECT next_at FROM workspace_recovery_budget WHERE session_id=?").bind(format!("wake-objective:{session}")).fetch_optional(pool).await?;
    if retry_at.is_some_and(|at| at > now) { anyhow::bail!("objective wake probe is waiting for bounded retry"); }
    // M255/R3-R5: the merge is independent of Objective retirement and of a
    // historical release ceiling. One admitted wake owns §8 for this PR.
    let merged = super::thin_derive::merged_closeout_fact(pool, session).await?;
    if let Some((reason, fires)) = merged.as_ref() {
        if fires == &0 {
            // Repair a legacy release-ceiling projection before admission.
            sqlx::query("UPDATE thin_external_observations SET completed_reason=?,closeout_queued=1 WHERE session_id=?").bind(reason).bind(session).execute(pool).await?;
            sqlx::query("UPDATE wakeup_schedules SET prompt=? WHERE session_id=? AND status='pending' AND (id LIKE 'thin-closeout-%' OR id LIKE 'thin-source-repair-%' OR id LIKE 'thin-repair-%' OR id LIKE 'thin-continue-%' OR id LIKE 'continue-%')")
                .bind(format!("{reason}。系统自动重试/自愈：只执行任务模板 §8 收尾，核验 squash commit、远端分支、本地状态并清理本任务产物与后台进程，然后报告真实结果；不要重复交付。"))
                .bind(session).execute(pool).await?;
            return Ok(false);
        }
        if crate::session_executor_alive(session) == Some(true) { return Ok(false); }
        if super::thin_derive::unanswered_user(pool, session).await? { return Ok(false); }
        if crate::commands::queued_delivery::has_fresh_pending(pool, session, now.saturating_mul(1000), QUEUED_MESSAGE_PRECEDENCE_SECS * 1000).await { return Ok(false); }
        let active: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM chat_run_controls WHERE session_id=? AND status IN ('active','cancel_requested')").bind(session).fetch_one(pool).await?;
        if active > 0 { return Ok(false); }
        let report = super::thin_derive::closeout_report(pool, session).await?;
        let result = if report.is_some() { format!("{reason}；§8 收尾完成，核验详情见本会话收尾报告与工具收据") }
            else { super::thin_derive::closeout_bounded_result(reason, *fires) };
        let state = super::thin_state::ThinState::ended(&result, "本会话记录");
        super::thin_state::save(pool, session, &state).await?;
        sqlx::query("UPDATE wakeup_schedules SET status='cancelled' WHERE session_id=? AND status='pending'").bind(session).execute(pool).await?;
        sqlx::query("UPDATE background_tasks SET wake_state='cancelled' WHERE session_id=? AND wake_state='pending'").bind(session).execute(pool).await?;
        return Ok(true);
    }
    // M251：升级重启会退休 objective，它不代表任务做完；PR 还开着就不能结束会话。
    if super::thin_derive::unfinished_delivery(pool, session).await?.is_some() { return Ok(false); }
    let mut tx = crate::storage::db::begin_write(pool).await?;
    let has_objectives: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='objectives'").fetch_one(&mut *tx).await?;
    if has_objectives == 0 { return Ok(false); }
    // 极简历史库没有目标准入事实，不把未知当作已完成。
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM objectives WHERE session_id=?").bind(session).fetch_one(&mut *tx).await?;
    if count == 0 { return Ok(false); }
    let unfinished: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM objectives WHERE session_id=? AND status NOT IN ('completed','cancelled','legacy_orphan')").bind(session).fetch_one(&mut *tx).await?;
    if unfinished > 0 || super::objective::open_delivery_objective_id_in_tx(&mut tx, session).await?.is_some() { return Ok(false); }
    sqlx::query("CREATE TABLE IF NOT EXISTS thin_system_wake_roots(root_turn_id TEXT PRIMARY KEY)").execute(&mut *tx).await?;
    let recovery: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM objectives objective WHERE objective.session_id=? AND objective.status IN ('completed','legacy_orphan') AND (
          EXISTS (SELECT 1 FROM messages user JOIN chat_turn_state turn ON turn.root_turn_id=user.id AND turn.session_id=user.session_id
            WHERE user.session_id=objective.session_id AND turn.objective_id=objective.id AND user.role='user'
              AND NOT EXISTS (SELECT 1 FROM thin_system_wake_roots wake WHERE wake.root_turn_id=user.id)
              AND NOT EXISTS (SELECT 1 FROM messages answer WHERE answer.session_id=user.session_id AND answer.role='assistant' AND answer.created_at>=user.created_at)
              AND NOT EXISTS (SELECT 1 FROM chat_session_cancel_intents stop WHERE stop.session_id=user.session_id AND stop.requested_at>=user.created_at))
          OR EXISTS (SELECT 1 FROM wakeup_schedules wake JOIN chat_turn_state turn ON turn.root_turn_id=wake.root_turn_id AND turn.session_id=wake.session_id
            JOIN thin_session_state thin ON thin.session_id=wake.session_id
            WHERE wake.session_id=objective.session_id AND turn.objective_id=objective.id AND wake.status='pending' AND wake.id LIKE 'model-availability-%' AND thin.state='waiting_wake'))")
        .bind(session).fetch_one(&mut *tx).await?;
    if recovery > 0 { return Ok(false); }
    // 用户新消息仍由正常入口处理；定时器不能结算正在执行的新回合。
    let active: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM chat_run_controls WHERE session_id=? AND status IN ('active','cancel_requested')").bind(session).fetch_one(&mut *tx).await?;
    if active > 0 || crate::session_executor_alive(session) == Some(true) { return Ok(false); }
    let reason = "会话没有未结算的 objective；迟到系统唤醒已取消，未启动新回合";
    sqlx::query("INSERT INTO thin_session_state(session_id,state,reason,outcome_location,updated_at) VALUES (?,'ended',?,'本会话记录',?) ON CONFLICT(session_id) DO UPDATE SET state='ended',reason=excluded.reason,waiting_for=NULL,wake_source=NULL,question=NULL,updated_at=excluded.updated_at")
        .bind(session).bind(reason).bind(now.saturating_mul(1000)).execute(&mut *tx).await?;
    sqlx::query("UPDATE wakeup_schedules SET status='cancelled' WHERE session_id=? AND status='pending'").bind(session).execute(&mut *tx).await?;
    sqlx::query("UPDATE background_tasks SET wake_state='cancelled' WHERE session_id=? AND wake_state='pending'").bind(session).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(true)
}

/// M241/R5：事实查询失败只延后，不耗尽后误判任务结束。
async fn defer_objective_probe(pool: &SqlitePool, session: &str, now: i64, error: &str) {
    if super::workspace_recovery_budget::ensure(pool).await.is_err() { return; }
    let key = format!("wake-objective:{session}");
    if let Err(error) = sqlx::query("INSERT INTO workspace_recovery_budget(session_id,attempts,next_at,last_error) VALUES (?,1,?+60,?) ON CONFLICT(session_id) DO UPDATE SET attempts=attempts+1,next_at=?+60*(1 << MIN(attempts,6)),last_error=excluded.last_error WHERE workspace_recovery_budget.next_at<=?")
        .bind(&key).bind(now).bind(error).bind(now).bind(now).execute(pool).await {
        tracing::warn!(%error, session, "objective wake probe backoff failed; automatic retry next tick");
    }
}

/// R2：会话停止/结案时取消其全部待触发唤醒（与后台任务回收同一入口）。
pub async fn cancel_session_wakeups(pool: &SqlitePool, session_id: &str) -> usize {
    if ensure_wakeup_schema(pool).await.is_err() {
        return 0;
    }
    let result = async {
        let mut tx = crate::storage::db::begin_write(pool).await?;
        let done = sqlx::query("UPDATE wakeup_schedules SET status='cancelled' WHERE session_id=? AND status='pending'")
            .bind(session_id).execute(&mut *tx).await?;
        // 停止/回收是明确终态；不能留下等一个已被取消的来源。
        let exists: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='thin_session_state'").fetch_one(&mut *tx).await?;
        if exists > 0 {
            sqlx::query("UPDATE thin_session_state SET state='ended',reason='cancelled：会话已停止，待触发唤醒已回收',waiting_for=NULL,wake_source=NULL,question=NULL,updated_at=? WHERE session_id=? AND state='waiting_wake'")
                .bind(now_secs()*1000).bind(session_id).execute(&mut *tx).await?;
        }
        tx.commit().await?;
        Ok::<_, sqlx::Error>(done)
    }.await;
    match result {
        Ok(done) => {
            let cancelled = done.rows_affected() as usize;
            if cancelled > 0 {
                tracing::info!(session = %session_id, cancelled, "session wakeups cancelled");
            }
            cancelled
        }
        Err(error) => {
            tracing::warn!(%error, session = %session_id, "session wakeups cancel failed");
            0
        }
    }
}

/// R2 / D10：唤醒文本固定形状——id + 提示原文 + 节奏（周期唤醒带序号）。
pub fn wakeup_text(wakeup: &WakeupSchedule, occurrence: i64, late: bool) -> String {
    let mut text = format!(
        "定时唤醒 {}：{}\n节奏：{}",
        wakeup.id,
        wakeup.prompt,
        wakeup.cadence()
    );
    if wakeup.every_sec.is_some() {
        text.push_str(&format!("（第 {occurrence} 次）"));
    }
    if late {
        text.push_str("\n计划时间已过（应用重启后补发），之后按原周期继续。");
    }
    text
}

/// R2 / D10：投递所有到点唤醒。
///
/// `now` 注入以便 headless 测试精确推进时间；生产由运行时循环传 `now_secs()`。
/// 投递失败（会话正在跑回合）保持 `pending`，下一轮重试；推进用
/// `next_at` 做比较并交换，因此并发/重启都不会重复投递同一次到点。
/// M205：恢复预算耗尽不是无限等待的许可。拒绝、终态与取消在同一事务提交。
async fn reject_exhausted_wakes(pool: &SqlitePool, session: &str, wakes: &[WakeupSchedule], now: i64) -> sqlx::Result<bool> {
    if super::thin_derive::unanswered_user(pool, session).await? { return Ok(false); }
    let budget: Option<(i64, Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT attempts,settled_reason,last_error FROM workspace_recovery_budget WHERE session_id=?")
        .bind(session).fetch_optional(pool).await?;
    let Some((attempts, settled, error)) = budget else { return Ok(false); };
    if attempts < 3 && settled.is_none() { return Ok(false); }
    // 活执行者仍拥有回合，绝不能被定时器结算。
    if crate::session_executor_alive(session) != Some(false) { return Ok(false); }
    if wake_source_cover(pool, session).await.running_tasks > 0 { return Ok(false); }
    super::thin_state::ensure(pool).await?;
    sqlx::query("CREATE TABLE IF NOT EXISTS session_audit_events(id TEXT PRIMARY KEY,session_id TEXT NOT NULL,kind TEXT NOT NULL,actor TEXT NOT NULL,detail_json TEXT NOT NULL,created_at INTEGER NOT NULL)").execute(pool).await?;
    let reason = format!("{}。系统自动恢复已结束：{}；本次唤醒未执行，不代表已完成 §8 文件清理。", wakes[0].prompt,
        settled.or(error).unwrap_or_else(|| "工作区恢复连续失败，已达到重试上限".into()));
    let mut tx = pool.begin().await?;
    sqlx::query("INSERT INTO thin_session_state(session_id,state,reason,outcome_location,updated_at) VALUES (?,'ended',?,'本会话记录',?) ON CONFLICT(session_id) DO UPDATE SET state='ended',reason=excluded.reason,waiting_for=NULL,wake_source=NULL,question=NULL,updated_at=excluded.updated_at WHERE thin_session_state.state<>'ended'")
        .bind(session).bind(&reason).bind(now.saturating_mul(1000)).execute(&mut *tx).await?;
    for wake in wakes {
        let detail = serde_json::json!({"wakeup_id":wake.id,"next_at":wake.next_at,"reason":reason,"attempts":attempts}).to_string();
        sqlx::query("INSERT OR IGNORE INTO session_audit_events(id,session_id,kind,actor,detail_json,created_at) VALUES (?,?,'wakeup_rejected','system',?,?)")
            .bind(format!("wake-rejected-{}-{}", wake.id, wake.next_at)).bind(session).bind(detail).bind(now.saturating_mul(1000)).execute(&mut *tx).await?;
    }
    sqlx::query("UPDATE wakeup_schedules SET status='cancelled' WHERE session_id=? AND status='pending'")
        .bind(session).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(true)
}

async fn record_wake_rejection(pool: &SqlitePool, session: &str, error: &str, now: i64) -> sqlx::Result<()> {
    let error = crate::trajectory::redact_text(error, 1000);
    super::thin_state::ensure(pool).await?;
    sqlx::query("CREATE TABLE IF NOT EXISTS session_audit_events(id TEXT PRIMARY KEY,session_id TEXT NOT NULL,kind TEXT NOT NULL,actor TEXT NOT NULL,detail_json TEXT NOT NULL,created_at INTEGER NOT NULL)").execute(pool).await?;
    let mut tx = crate::storage::db::begin_write(pool).await?;
    let attempts: i64 = sqlx::query_scalar("INSERT INTO workspace_recovery_budget(session_id,attempts,next_at,last_error) VALUES (?,1,?,?) ON CONFLICT(session_id) DO UPDATE SET attempts=attempts+1,next_at=excluded.next_at,last_error=excluded.last_error RETURNING attempts")
        .bind(format!("wake-delivery:{session}")).bind(now + 60).bind(&error).fetch_one(&mut *tx).await?;
    let detail = serde_json::json!({"error":error,"attempts":attempts,"next_at":now+60}).to_string();
    sqlx::query("INSERT INTO session_audit_events(id,session_id,kind,actor,detail_json,created_at) VALUES (?,?,'wakeup_admission_failed','system',?,?)")
        .bind(uuid::Uuid::new_v4().to_string()).bind(session).bind(detail).bind(now*1000).execute(&mut *tx).await?;
    if attempts >= 30 {
        let reason = format!("自动继续失败：{error}。系统已每分钟重试唤醒准入，共 {attempts} 次，自动恢复已结束；任务未完成，详情见本会话审计记录。");
        sqlx::query("UPDATE thin_session_state SET state='ended',reason=?,waiting_for=NULL,wake_source=NULL,question=NULL,updated_at=? WHERE session_id=?")
            .bind(reason).bind(now*1000).bind(session).execute(&mut *tx).await?;
        sqlx::query("UPDATE wakeup_schedules SET status='cancelled' WHERE session_id=? AND status='pending'").bind(session).execute(&mut *tx).await?;
    } else if attempts >= 5 {
        let notice = format!("自动继续失败：{error}；系统自动重试/自愈，将在 1 分钟后重试（已尝试 {attempts} 次）");
        sqlx::query("UPDATE thin_session_state SET waiting_for=?,reason=?,updated_at=? WHERE session_id=? AND state='waiting_wake'")
            .bind(&notice).bind(&notice).bind(now*1000).bind(session).execute(&mut *tx).await?;
    }
    tx.commit().await
}

/// R3 precedence window: a queued user message younger than this defers the
/// wake-up. Older rows stop blocking so a stuck message cannot freeze the
/// schedule forever (it stays visible as a queue row / failure).
pub const QUEUED_MESSAGE_PRECEDENCE_SECS: i64 = 600;

pub async fn drain_due_wakeups_at(pool: &SqlitePool, sink: &dyn WakeSink, now: i64) -> usize {
    use futures_util::{stream::FuturesUnordered, StreamExt};
    // 同一进程只有一个扫描器可推进计划；先锁再读，不能依赖入队后的 CAS 去重。
    static SCANS: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<usize, std::sync::Weak<tokio::sync::Mutex<()>>>>> = std::sync::OnceLock::new();
    let key = std::sync::Arc::as_ptr(&pool.connect_options()) as usize;
    let scan = {
        let mut scans = SCANS.get_or_init(Default::default).lock().unwrap_or_else(|poison| poison.into_inner());
        scans.retain(|_, value| value.strong_count() > 0);
        let scan = scans.get(&key).and_then(std::sync::Weak::upgrade)
            .unwrap_or_else(|| std::sync::Arc::new(tokio::sync::Mutex::new(())));
        scans.insert(key, std::sync::Arc::downgrade(&scan));
        scan
    };
    let Ok(_scan) = scan.try_lock() else { return 0; };
    if ensure_wakeup_schema(pool).await.is_err() { return 0; }
    let rows = match sqlx::query(
        "SELECT * FROM wakeup_schedules WHERE status='pending' AND next_at<=? ORDER BY next_at,id",
    ).bind(now).fetch_all(pool).await {
        Ok(rows) => rows,
        Err(error) => { tracing::warn!(%error, "due wake scan failed; automatic retry next tick"); return 0; }
    };
    let mut sessions = std::collections::BTreeMap::<String, Vec<WakeupSchedule>>::new();
    for row in rows {
        let wake = row_to_wakeup(&row);
        sessions.entry(wake.session_id.clone()).or_default().push(wake);
    }
    let mut pending = FuturesUnordered::new();
    for (session, wakes) in sessions {
        pending.push(async move {
            if let Ok(Some(state)) = super::thin_state::load(pool, &session).await {
                if state.state == super::thin_state::SessionState::Ended {
                    cancel_session_wakeups(pool, &session).await;
                    return 0;
                }
            }
            match settle_objectiveless_wake(pool, &session, now).await {
                Ok(true) => return 0,
                Ok(false) => {},
                Err(error) => {
                    defer_objective_probe(pool, &session, now, &error.to_string()).await;
                    return 0;
                }
            }
            // M237：模型恢复前投递只会写一条无人回答的消息。顺延到模型唤醒，到点合并为一个回合。
            if hold_for_model_wake(pool, &session, &wakes, now).await { return 0; }
            // M216：旧版本把准入失败记到了工作区预算，迁移时仅清除这类未结算技术记录。
            if let Err(error) = sqlx::query("DELETE FROM workspace_recovery_budget WHERE session_id=? AND settled_reason IS NULL AND last_error='唤醒投递失败；系统自动重试/自愈'")
                .bind(&session).execute(pool).await {
                // 表尚未建立时由下面的 due 建立；其他错误同样留待下轮重试。
                tracing::debug!(%error, session, "legacy admission budget repair deferred");
            }
            match super::workspace_recovery_budget::due(pool, &session, now).await {
                Ok(true) => {},
                Ok(false) => {
                    // M209: a prior workspace conclusion cannot veto an unanswered
                    // task. Delivery has its own backoff so retries cannot spin.
                    if super::thin_derive::unanswered_user(pool, &session).await.unwrap_or(false) {
                        let delivery_key = format!("wake-delivery:{session}");
                        super::workspace_recovery_budget::ensure(pool).await.ok();
                        let next: Option<i64> = sqlx::query_scalar("SELECT next_at FROM workspace_recovery_budget WHERE session_id=?")
                            .bind(&delivery_key).fetch_optional(pool).await.unwrap_or(Some(i64::MAX));
                        if next.is_some_and(|next| next > now) { return 0; }
                    } else {
                        if let Err(error) = reject_exhausted_wakes(pool, &session, &wakes, now).await {
                            tracing::warn!(%error, session, "wake rejection persistence failed; system automatic recovery next tick");
                        }
                        return 0;
                    }
                },
                Err(error) => {
                    tracing::warn!(%error, session, "wake budget unavailable; system automatic recovery next tick");
                    return 0;
                }
            }
            // M216：准入失败属于传输重试，不是工作区恢复失败，不能耗尽/取消修复计划。
            let delivery_key = format!("wake-delivery:{session}");
            if super::workspace_recovery_budget::ensure(pool).await.is_err() { return 0; }
            let retry_at: Result<Option<i64>, _> = sqlx::query_scalar("SELECT next_at FROM workspace_recovery_budget WHERE session_id=?")
                .bind(&delivery_key).fetch_optional(pool).await;
            match retry_at { Ok(Some(at)) if at > now => return 0, Err(_) => return 0, _ => {} }
            let due = wakes.iter().map(|wake| wake.next_at).min().unwrap_or(now);
            if now.saturating_sub(due) > 120 {
                tracing::warn!(session, pending_seconds = now.saturating_sub(due), "wake overdue over two minutes; automatic retry");
                let _ = sqlx::query("INSERT INTO wakeup_alerts(session_id,first_seen_at,last_retry_at,delay_secs,retry_count) VALUES (?,?,?,?,1) ON CONFLICT(session_id) DO UPDATE SET last_retry_at=excluded.last_retry_at,delay_secs=excluded.delay_secs,retry_count=wakeup_alerts.retry_count+1")
                    .bind(&session).bind(now).bind(now).bind(now.saturating_sub(due)).execute(pool).await;
            }
            // M236/R3：用户消息优先于系统唤醒。会话里还有新入队的用户消息时，
            // 本次唤醒不起回合（也不推进 next_at），让那条消息先投递；
            // 唤醒计划保持 pending，等它结算后下一轮扫描继续投递。
            if crate::commands::queued_delivery::has_fresh_pending(
                pool,
                &session,
                now.saturating_mul(1000),
                QUEUED_MESSAGE_PRECEDENCE_SECS * 1000,
            )
            .await
            {
                tracing::info!(session, "queued user message takes precedence over this wake-up");
                return 0;
            }
            let text = if let Ok(Some((reason, _))) = super::thin_derive::merged_closeout_fact(pool, &session).await {
                format!("{reason}。系统自动重试/自愈：现在只执行任务模板 §8 收尾，确认 squash commit、检查并清理远端分支、检查本地未提交/未推送状态并清理本任务构建产物与后台进程；报告真实结果，不重复交付。")
            } else {
                wakes.iter().map(|wake| {
                    wakeup_text(wake, wake.fire_count + 1, now.saturating_sub(wake.next_at) >= wake.every_sec.unwrap_or(1).max(1) as i64)
                }).collect::<Vec<_>>().join("\n\n")
            };
            let delivery = tokio::time::timeout(Duration::from_secs(5), sink.enqueue_detailed(session.clone(), text)).await
                .unwrap_or_else(|_| Err("唤醒准入超时（5 秒）".into()));
            if let Err(error) = delivery {
                if let Err(error) = record_wake_rejection(pool, &session, &error, now).await {
                    tracing::warn!(%error, session, "wake retry persistence failed; automatic retry next tick");
                }
                return 0;
            }
            // Ok(false) means the active executor owns the session: defer without
            // charging a failed-admission attempt or ending that live turn.
            if delivery == Ok(false) { return 0; }
            let _ = super::workspace_recovery_budget::clear(pool, &session).await;
            let _ = super::workspace_recovery_budget::clear(pool, &format!("wake-delivery:{session}")).await;
            for wake in &wakes {
                let cycle = wake.every_sec.or(wake.after_sec).unwrap_or(60).max(1) as i64;
                if now.saturating_sub(wake.next_at) > cycle.saturating_mul(2) {
                    let detail = serde_json::json!({"wakeup_id":wake.id,"next_at":wake.next_at,"recovered_at":now,"delay_seconds":now.saturating_sub(wake.next_at)}).to_string();
                    if let Err(error) = sqlx::query("INSERT OR IGNORE INTO session_audit_events(id,session_id,kind,actor,detail_json,created_at) VALUES (?,?,'wakeup_overdue_recovered','system',?,?)")
                        .bind(format!("wake-overdue-{}-{}", wake.id, wake.next_at)).bind(&session).bind(detail).bind(now.saturating_mul(1000)).execute(pool).await {
                        tracing::warn!(%error, session, "overdue wake audit deferred; system automatic recovery");
                    }
                }
            }
            // 至少一次：成功入队才推进；崩溃前未推进仍可重试，不丢失原因。
            let mut advanced = false;
            for wake in wakes {
                let result = sqlx::query("UPDATE wakeup_schedules SET status=?,next_at=?,fire_count=fire_count+1,last_fired_at=? WHERE id=? AND status='pending' AND next_at=?")
                    .bind(if wake.every_sec.is_some() { "pending" } else { "done" })
                    .bind(wake.every_sec.map_or(wake.next_at, |every| now.saturating_add(every as i64)))
                    .bind(now).bind(&wake.id).bind(wake.next_at).execute(pool).await;
                match result {
                    Ok(done) => advanced |= done.rows_affected() == 1,
                    Err(error) => tracing::warn!(%error, wakeup = %wake.id, "wake receipt deferred; automatic retry"),
                }
            }
            usize::from(advanced)
        });
    }
    let mut delivered = 0;
    while let Some(count) = pending.next().await { delivered += count; }
    delivered
}

/// M237：会话正在等模型恢复时，把到点的其它唤醒顺延到恢复时间。不计 `fire_count`。
async fn hold_for_model_wake(pool: &SqlitePool, session: &str, wakes: &[WakeupSchedule], now: i64) -> bool {
    if wakes.iter().any(|wake| wake.id.starts_with("model-availability-")) { return false; }
    let resume_at: Option<i64> = match sqlx::query_scalar(
        "SELECT MIN(next_at) FROM wakeup_schedules WHERE session_id=? AND status='pending' AND id LIKE 'model-availability-%' AND next_at>?",
    ).bind(session).bind(now).fetch_one(pool).await {
        Ok(resume_at) => resume_at,
        Err(error) => {
            tracing::warn!(%error, session, "model wake lookup failed; delivering due wakes");
            return false;
        }
    };
    let Some(resume_at) = resume_at else { return false; };
    for wake in wakes {
        if let Err(error) = sqlx::query("UPDATE wakeup_schedules SET next_at=? WHERE id=? AND status='pending' AND next_at=?")
            .bind(resume_at).bind(&wake.id).bind(wake.next_at).execute(pool).await {
            tracing::warn!(%error, wakeup = %wake.id, "wake hold deferred; automatic retry next tick");
        }
    }
    true
}

/// 生产入口：用当前时间投递到点唤醒。
pub async fn drain_due_wakeups(pool: &SqlitePool, sink: &dyn WakeSink) -> usize {
    drain_due_wakeups_at(pool, sink, now_secs()).await
}

// ── R5：结束回合前的唤醒源门禁查询 ────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct WakeSourceCover {
    pub running_tasks: usize,
    pub pending_wakeups: usize,
    /// M197/R2：系统按有界心跳（≥180s、遵守 GitHub 配额）观测的 PR/CI 事项。
    /// 它本身就是 durable 唤醒源——心跳会把它推进到「合并/检查全绿/失败」，
    /// 因此不能把「刚开完 PR 的正常等待」判成无源等待。
    pub external_observations: usize,
}

impl WakeSourceCover {
    pub fn is_covered(&self) -> bool {
        self.running_tasks + self.pending_wakeups + self.external_observations > 0
    }

    /// 缺唤醒源时给 agent 的一句可执行提示（不针对用户、不进对话）。
    pub fn missing_hint(&self) -> Option<String> {
        if self.is_covered() {
            return None;
        }
        Some(
            "结束这个等待前先挂一个唤醒源：background_task_start（等待的命令结束时唤醒本会话）\
             或 schedule_wakeup（到点唤醒）。没有唤醒源就没人会把你叫回来继续收尾。\
             等 CI 时推荐 `background_task_start` + `gh pr checks <n> --watch`；\
             如果要自己轮询，间隔必须 ≥180s（M114：GitHub API 额度）。"
                .to_string(),
        )
    }
}

/// R5 / D11：本会话当前的唤醒源（运行中的后台任务 + 待触发的定时唤醒）。
pub async fn wake_source_cover(pool: &SqlitePool, session_id: &str) -> WakeSourceCover {
    if ensure_schema(pool).await.is_err() {
        return WakeSourceCover {
            running_tasks: 0,
            pending_wakeups: 0,
            external_observations: 0,
        };
    }
    let running_tasks: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM background_tasks WHERE session_id = ? AND status = 'running'",
    )
    .bind(session_id)
    .fetch_one(pool)
    .await
    .unwrap_or(0);
    let pending_wakeups: i64 = if ensure_wakeup_schema(pool).await.is_ok() {
        sqlx::query_scalar(
            "SELECT COUNT(*) FROM wakeup_schedules WHERE session_id = ? AND status = 'pending'",
        )
        .bind(session_id)
        .fetch_one(pool)
        .await
        .unwrap_or(0)
    } else {
        0
    };
    let external_observations: i64 = if ensure_external_table(pool).await.is_ok() {
        sqlx::query_scalar(
            "SELECT COUNT(*) FROM thin_external_observations WHERE session_id = ? AND completed_reason IS NULL",
        )
        .bind(session_id)
        .fetch_one(pool)
        .await
        .unwrap_or(0)
    } else {
        0
    };
    WakeSourceCover {
        running_tasks: running_tasks.max(0) as usize,
        pending_wakeups: pending_wakeups.max(0) as usize,
        external_observations: external_observations.max(0) as usize,
    }
}

/// M197/R2：观测表由 thin_derive 建立；这里只读，不存在就当没有源。
async fn ensure_external_table(pool: &SqlitePool) -> sqlx::Result<()> {
    let exists: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='thin_external_observations'",
    )
    .fetch_one(pool)
    .await?;
    if exists == 0 {
        return Err(sqlx::Error::RowNotFound);
    }
    Ok(())
}

/// R5：只约束仍有外部等待的回合，不改变 objective 或交付状态。
pub async fn external_wait_completion_hint(
    pool: &SqlitePool,
    session_id: &str,
    external_wait_pending: bool,
) -> Option<String> {
    if !external_wait_pending {
        return None;
    }
    wake_source_cover(pool, session_id).await.missing_hint()
}

// ── 应用装配（非测试构建） ────────────────────────────────────────────────────

/// 后台任务输出目录：应用数据目录内，绝不写工作区。
#[cfg(not(test))]
pub fn background_dir(app: &tauri::AppHandle) -> PathBuf {
    use tauri::Manager;
    app.path()
        .app_data_dir()
        .map(|dir| dir.join("background-tasks"))
        .unwrap_or_else(|_| std::env::temp_dir().join("codefactory-background-tasks"))
}

/// 启动对账 + 唤醒投递循环。应用启动时调用一次。
#[cfg(not(test))]
pub fn start_runtime(app: tauri::AppHandle) {
    tauri::async_runtime::spawn(async move {
        use tauri::Manager;
        let pool = {
            let state = app.state::<crate::AppState>();
            let pool = state.db.read().await.clone();
            pool
        };
        if let Err(error) = ensure_schema(&pool).await {
            tracing::warn!(%error, "background task schema unavailable");
            return;
        }
        // M205：对账含进程恢复/外部观测，不能成为首次到期扫描的前置屏障。
        let reconcile_pool = pool.clone();
        tauri::async_runtime::spawn(async move {
            match reconcile(&reconcile_pool).await {
                Ok(report) => tracing::info!(resumed = report.resumed, finished = report.finished, lost = report.lost, "background task reconcile finished"),
                Err(error) => tracing::warn!(%error, "background task reconcile failed"),
            }
        });
        let sink = ChatWakeSink { app: app.clone() };
        let patrol_pool = pool.clone();
        tauri::async_runtime::spawn(async move {
            loop {
                if let Err(error) = crate::agent::thin_derive::patrol_waiting_wake_sources(&patrol_pool).await {
                    tracing::warn!(%error, "local wake patrol failed; automatic retry next tick");
                }
                tokio::time::sleep(Duration::from_secs(30)).await;
            }
        });
        let guard_pool = pool.clone();
        tauri::async_runtime::spawn(async move {
            loop {
                match crate::agent::thin_derive::reconcile_waiting_wake_sources(&guard_pool).await {
                    Ok(report) => {
                        if report.swept > 0 || report.still_waiting != report.scanned {
                            tracing::info!(
                                scanned = report.scanned,
                                swept = report.swept,
                                still_waiting = report.still_waiting,
                                "waiting_wake reconcile tick"
                            );
                        }
                    }
                    Err(error) => tracing::warn!(%error, "waiting_wake reconcile failed"),
                }
                tokio::time::sleep(Duration::from_secs(180)).await;
            }
        });
        loop {
            // 两类投递并发：后台命令积压不能阻塞定时唤醒扫描。
            let (delivered, fired) = tokio::join!(
                drain_pending_wakes(&pool, &sink),
                drain_due_wakeups(&pool, &sink),
            );
            if delivered + fired > 0 {
                tracing::info!(delivered, fired, "durable wakes enqueued");
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    });
}

/// 测试构建不装配运行时（headless 测试直接调用 reconcile/drain）。
#[cfg(test)]
pub fn start_runtime(_app: tauri::AppHandle) {}

#[cfg(not(test))]
struct ChatWakeSink {
    app: tauri::AppHandle,
}

#[cfg(not(test))]
impl WakeSink for ChatWakeSink {
    fn enqueue(&self, session_id: String, text: String) -> WakeFuture {
        let future = self.enqueue_detailed(session_id, text);
        Box::pin(async move { future.await == Ok(true) })
    }

    fn enqueue_detailed(&self, session_id: String, text: String) -> Pin<Box<dyn std::future::Future<Output = Result<bool, String>> + Send>> {
        let app = self.app.clone();
        Box::pin(async move {
            match crate::commands::chat::send_system_wake(app, session_id, text).await {
                Ok(()) => Ok(true),
                Err(error) if error.to_string().contains("CHAT_RUN_BUSY") => Ok(false),
                Err(error) => Err(crate::trajectory::redact_text(&error.to_string(), 1000)),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex as StdMutex};

    #[test]
    fn m143_background_log_decodes_chinese_at_character_boundaries() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("output.log");
        std::fs::write(&path, "prefix中文🙂").unwrap();
        assert_eq!(output_tail(&path, 4), "🙂");
        #[cfg(windows)]
        {
            std::fs::write(&path, [0xd6, 0xd0, 0xce, 0xc4]).unwrap();
            // Legacy bytes are tested deterministically independent of CI's ACP.
            assert_eq!(crate::util::command_env::decode_output_for_code_page(&std::fs::read(&path).unwrap(), 936), "中文");
        }
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn m143_windows_background_chinese_and_legacy_parser_failure() {
        let pool = memory_pool().await;
        let dir = tempfile::tempdir().unwrap();
        let started = start(&pool, request("m143", dir.path(), "echo 中文", Some(30))).await.unwrap();
        let task = wait_terminal(&pool, &started.id, 40).await;
        assert_eq!(task.exit_code, Some(0));
        assert_eq!(output_tail(Path::new(&task.output_path), 1000).trim(), "中文");
        let inv = crate::util::command_env::windows_shell_invocation("echo first && echo second", false, true);
        let mut command = std::process::Command::new(inv.program).no_window();
        command.args(inv.args);
        let output = command.output().unwrap();
        assert!(!output.status.success());
        let text = crate::util::command_env::decode_output(&output.stderr);
        assert!(!text.contains('\u{fffd}'), "{text}");
        assert!(crate::util::command_env::shell_failure_hint("powershell.exe", &text).contains("5.1"));
    }

    async fn memory_pool() -> SqlitePool {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        ensure_schema(&pool).await.unwrap();
        pool
    }

    fn scratch_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "cf-bg-{tag}-{}-{:?}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn request(session: &str, dir: &Path, command: &str, timeout_sec: Option<u64>) -> StartRequest {
        StartRequest {
            session_id: session.to_string(),
            root_turn_id: Some("turn-1".to_string()),
            cwd: dir.to_path_buf(),
            output_dir: dir.join("logs"),
            command: command.to_string(),
            source: TaskSource::AgentDecided,
            timeout_sec,
        }
    }

    async fn wait_terminal(pool: &SqlitePool, id: &str, secs: u64) -> BackgroundTask {
        let started = std::time::Instant::now();
        loop {
            let task = get(pool, id).await.unwrap().unwrap();
            if task.is_terminal() {
                return task;
            }
            assert!(
                started.elapsed().as_secs() < secs,
                "background task {id} never reached a terminal state"
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    #[tokio::test]
    async fn m183_cross_process_overdue_wakes_are_coalesced_without_starvation() {
        const CHILD: &str = "CODEFACTORY_M183_SYNTHETIC_DB";
        if let Ok(path) = std::env::var(CHILD) {
            let pool = file_pool(Path::new(&path)).await;
            let recorder = Arc::new(Recorder::default());
            assert_eq!(drain_due_wakeups_at(&pool, sink(&recorder), 10_000).await, 2);
            let delivered = recorder.take();
            assert_eq!(delivered.len(), 2, "one enqueue per session, not per wake row");
            let combined = &delivered.iter().find(|(session, _)| session == "synthetic-many").unwrap().1;
            for index in 0..40 {
                assert!(combined.contains(&format!("reason-{index:02}")), "all overdue reasons must survive");
            }
            assert_eq!(drain_due_wakeups_at(&pool, sink(&recorder), 10_000).await, 0);
            assert_eq!(drain_due_wakeups_at(&pool, sink(&recorder), 10_060).await, 1);
            assert!(recorder.take()[0].1.contains("future reason"));
            let done: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM wakeup_schedules WHERE status='done' AND fire_count=1")
                .fetch_one(&pool).await.unwrap();
            assert_eq!(done, 42);
            pool.close().await;
            return;
        }
        let dir = scratch_dir("m183-process-restart");
        let pool = file_pool(&dir).await;
        for index in 0..40 {
            schedule_wakeup_at(&pool, wakeup_request("synthetic-many", &format!("reason-{index:02}"), None), 100).await.unwrap();
            // Simulate historical backlog beyond the per-session tool cap.
            sqlx::query("UPDATE wakeup_schedules SET session_id='synthetic-staging' WHERE session_id='synthetic-many'")
                .execute(&pool).await.unwrap();
        }
        sqlx::query("UPDATE wakeup_schedules SET session_id='synthetic-many'").execute(&pool).await.unwrap();
        schedule_wakeup_at(&pool, wakeup_request("synthetic-other", "other reason", None), 100).await.unwrap();
        schedule_wakeup_at(&pool, wakeup_request("synthetic-future", "future reason", None), 10_000).await.unwrap();
        pool.close().await;
        let child = tokio::process::Command::new(std::env::current_exe().unwrap())
            .args(["m183_cross_process_overdue_wakes_are_coalesced_without_starvation", "--nocapture"])
            .env(CHILD, &dir).output();
        let output = tokio::time::timeout(Duration::from_secs(30), child).await.unwrap().unwrap();
        assert!(output.status.success(), "child failed: {} {}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn m184_failed_wakes_do_not_hot_loop() {
        struct Reject(std::sync::atomic::AtomicUsize);
        impl WakeSink for Reject {
            fn enqueue(&self, _: String, _: String) -> WakeFuture {
                self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Box::pin(async { false })
            }
        }
        let pool = memory_pool().await;
        ensure_wakeup_schema(&pool).await.unwrap();
        sqlx::query("INSERT INTO wakeup_schedules(id,session_id,prompt,source,next_at,status,created_at) VALUES ('synthetic-retry','s1','retry','agent_decided',1,'pending',1)").execute(&pool).await.unwrap();
        let sink = Reject(std::sync::atomic::AtomicUsize::new(0));
        for at in 1000..1060 { drain_due_wakeups_at(&pool, &sink, at).await; }
        assert_eq!(sink.0.load(std::sync::atomic::Ordering::SeqCst), 1, "at most one failure/log per minute per session");
        drain_due_wakeups_at(&pool, &sink, 1060).await;
        assert_eq!(sink.0.load(std::sync::atomic::Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn m183_blocked_sessions_do_not_delay_a_healthy_session() {
        let pool = memory_pool().await;
        for index in 0..7 {
            schedule_wakeup_at(&pool, wakeup_request(&format!("blocked-{index}"), "blocked", None), 100).await.unwrap();
        }
        schedule_wakeup_at(&pool, wakeup_request("healthy", "healthy reason", None), 101).await.unwrap();
        struct Mixed(Arc<Recorder>);
        impl WakeSink for Mixed {
            fn enqueue(&self, session: String, text: String) -> WakeFuture {
                if session.starts_with("blocked-") { return Box::pin(std::future::pending()); }
                self.0.enqueue(session, text)
            }
        }
        let recorder = Arc::new(Recorder::default());
        let result = tokio::time::timeout(Duration::from_secs(7), drain_due_wakeups_at(&pool, &Mixed(recorder.clone()), 200)).await;
        assert_eq!(result.expect("blocked sessions must run concurrently, not consume 35 seconds"), 1);
        assert_eq!(recorder.take()[0].0, "healthy");
    }

    #[tokio::test]
    async fn m183_concurrent_scans_enqueue_a_session_only_once() {
        let pool = memory_pool().await;
        schedule_wakeup_at(&pool, wakeup_request("synthetic", "once", None), 100).await.unwrap();
        struct Slow(Arc<Recorder>);
        impl WakeSink for Slow {
            fn enqueue(&self, session: String, text: String) -> WakeFuture {
                let recorder = self.0.clone();
                Box::pin(async move {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    recorder.enqueue(session, text).await
                })
            }
        }
        let recorder = Arc::new(Recorder::default());
        let slow = Slow(recorder.clone());
        let (first, second) = tokio::join!(drain_due_wakeups_at(&pool, &slow, 200), drain_due_wakeups_at(&pool, &slow, 200));
        assert_eq!(first + second, 1);
        assert_eq!(recorder.take().len(), 1, "CAS after enqueue cannot prevent duplicate side effects");
    }

    #[tokio::test]
    async fn m168_a_blocked_sink_cannot_stall_pending_wakes() {
        struct Blocked;
        impl WakeSink for Blocked {
            fn enqueue(&self, _: String, _: String) -> WakeFuture {
                Box::pin(std::future::pending())
            }
        }
        let pool = memory_pool().await;
        let wake = schedule_wakeup_at(&pool, WakeupRequest {
            session_id: "m168-synthetic".into(), root_turn_id: None,
            prompt: "检查窗口恢复".into(), source: TaskSource::AgentDecided,
            after_sec: Some(60), every_sec: None,
        }, 100).await.unwrap();
        let result = tokio::time::timeout(Duration::from_secs(7),
            drain_due_wakeups_at(&pool, &Blocked, 160)).await;
        assert!(result.is_ok(), "one blocked enqueue must not stop the durable dispatcher");
        assert_eq!(get_wakeup(&pool, &wake.id).await.unwrap().unwrap().status, "pending");
        let recorder = Arc::new(Recorder::default());
        assert_eq!(drain_due_wakeups_at(&pool, sink(&recorder), 161).await, 0);
        assert_eq!(drain_due_wakeups_at(&pool, sink(&recorder), 220).await, 1);
        assert_eq!(recorder.take().len(), 1);
    }

    async fn m241_fixture(status: &str, prefix: &str) -> SqlitePool {
        let pool = crate::storage::db::connect("sqlite::memory:").await.unwrap();
        ensure_schema(&pool).await.unwrap();
        super::super::thin_state::ensure(&pool).await.unwrap();
        sqlx::query("INSERT INTO sessions(id,title,cwd,model_id,created_at,updated_at) VALUES ('m241','synthetic','/synthetic','model',1,1)").execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO chat_turn_state(root_turn_id,session_id,phase,status,started_at,updated_at) VALUES ('original','m241','working','active',1,1),('fresh-user','m241','working','active',1,1)").execute(&pool).await.unwrap();
        let objective = super::super::objective::ObjectiveStore::new(pool.clone())
            .ensure_chat_objective("m241", "original", super::super::objective::ObjectiveKind::Delivery, "delivery_receipt").await.unwrap();
        sqlx::query("UPDATE objectives SET status=?,decision_type=CASE WHEN ?='completed' THEN 'complete' WHEN ?='cancelled' THEN 'cancelled' ELSE decision_type END,cancellation_provenance=CASE WHEN ?='cancelled' THEN 'explicit_cancel' ELSE NULL END,evidence_ref='synthetic-receipt',reached_acceptance='delivery_receipt',completed_at=1,recovery_owner=NULL,remediation_id=NULL,lease_owner=NULL,lease_expires_at=NULL WHERE id=?").bind(status).bind(status).bind(status).bind(status).bind(objective.id).execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO thin_session_state(session_id,state,waiting_for,wake_source,updated_at) VALUES ('m241','waiting_wake','PR repair','pending wake',1)").execute(&pool).await.unwrap();
        for (suffix, at) in [("due", 100_i64), ("future", 10000)] {
            sqlx::query("INSERT INTO wakeup_schedules(id,session_id,prompt,source,next_at,status,created_at) VALUES (?,'m241','synthetic wake','agent_decided',?,'pending',1)")
                .bind(format!("{prefix}-{suffix}")).bind(at).execute(&pool).await.unwrap();
        }
        pool
    }

    #[tokio::test]
    async fn m241_settled_objective_cancels_wake_family_without_admission() {
        for status in ["completed", "cancelled"] {
            for prefix in ["thin-repair", "thin-closeout", "thin-source-repair", "continue", "model-availability", "scheduled"] {
                let pool = m241_fixture(status, prefix).await;
                let workspaces: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM execution_workspaces").fetch_one(&pool).await.unwrap();
                let recorder = Arc::new(Recorder::default());
                assert_eq!(drain_due_wakeups_at(&pool, sink(&recorder), 200).await, 0, "{status}/{prefix}");
                assert!(recorder.take().is_empty(), "no turn admission");
                let state = super::super::thin_state::load(&pool, "m241").await.unwrap().unwrap();
                assert_eq!(state.state, super::super::thin_state::SessionState::Ended);
                assert!(state.reason.unwrap().contains("没有未结算"));
                assert!(state.waiting_for.is_none() && state.wake_source.is_none());
                assert_eq!(sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM wakeup_schedules WHERE status='cancelled'").fetch_one(&pool).await.unwrap(), 2);
                assert_eq!(sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM messages WHERE role='user'").fetch_one(&pool).await.unwrap(), 0);
                assert_eq!(sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM objectives").fetch_one(&pool).await.unwrap(), 1);
                assert_eq!(sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM execution_workspaces").fetch_one(&pool).await.unwrap(), workspaces);
                assert_eq!(drain_due_wakeups_at(&pool, sink(&recorder), 20000).await, 0);
                let next = super::super::objective::ObjectiveStore::new(pool.clone())
                    .ensure_or_continue_chat_objective("m241", "fresh-user", None, super::super::objective::ObjectiveKind::Delivery, "delivery_receipt").await.unwrap();
                assert_ne!(next.status.as_str(), "completed");
                assert_eq!(sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM objectives").fetch_one(&pool).await.unwrap(), 2);
            }
        }
    }

    // M251: an upgrade restart retires objectives, so "no unsettled objective"
    // alone must not end a session whose PR is still open.
    #[tokio::test]
    async fn m255_merged_orphan_gets_one_closeout_across_wake_family() {
        for prefix in ["thin-closeout", "thin-source-repair", "thin-repair", "thin-continue"] {
            let pool = m241_fixture("legacy_orphan", prefix).await;
            // The original task predates its merge; a newer task must stay protected.
            sqlx::query("UPDATE objectives SET created_at=1 WHERE session_id='m241'").execute(&pool).await.unwrap();
            sqlx::query("CREATE TABLE IF NOT EXISTS thin_delivery_refs(session_id TEXT PRIMARY KEY,branch TEXT NOT NULL,pr_number INTEGER NOT NULL,ceiling TEXT NOT NULL)").execute(&pool).await.unwrap();
            sqlx::query("CREATE TABLE IF NOT EXISTS thin_external_observations(session_id TEXT PRIMARY KEY,pr_number INTEGER NOT NULL,ceiling TEXT NOT NULL,observed_at INTEGER NOT NULL,completed_reason TEXT,closeout_queued INTEGER,merged_at INTEGER)").execute(&pool).await.unwrap();
            sqlx::query("INSERT INTO thin_delivery_refs VALUES ('m241','synthetic',1002,'merged')").execute(&pool).await.unwrap();
            sqlx::query("INSERT INTO thin_external_observations VALUES ('m241',1002,'merged',100000,'PR #1002 已合并',1,100000)").execute(&pool).await.unwrap();
            assert!(!settle_objectiveless_wake(&pool, "m241", 200).await.unwrap(), "{prefix}: retired is not closed out");
            let recorder = Arc::new(Recorder::default());
            assert_eq!(drain_due_wakeups_at(&pool, sink(&recorder), 200).await, 1, "{prefix}");
            let delivered = recorder.take();
            assert_eq!(delivered.len(), 1);
            assert!(delivered[0].1.contains("§8 收尾") && delivered[0].1.contains("不重复交付"));
            let store = super::super::objective::ObjectiveStore::new(pool.clone());
            let mut tx = crate::storage::db::begin_write(&pool).await.unwrap();
            let admitted = store.ensure_or_continue_chat_objective_in_tx_with_origin(
                &mut tx, "m241", "fresh-user", None,
                super::super::objective::ObjectiveKind::Delivery, "delivery_receipt",
                super::super::objective::TurnOrigin::SystemWake,
            ).await.unwrap();
            tx.commit().await.unwrap();
            assert_eq!(sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM objectives WHERE session_id='m241'").fetch_one(&pool).await.unwrap(), 1);
            assert_eq!(admitted.status.as_str(), "active");
            sqlx::query("INSERT INTO messages(id,session_id,role,content,created_at) VALUES ('answer','m241','assistant','已完成',201000)").execute(&pool).await.unwrap();
            assert!(settle_objectiveless_wake(&pool, "m241", 300).await.unwrap(), "{prefix}: no-tool answer cannot spin");
            assert_eq!(drain_due_wakeups_at(&pool, sink(&recorder), 20000).await, 0);
            let state = super::super::thin_state::load(&pool, "m241").await.unwrap().unwrap();
            assert_eq!(state.state, super::super::thin_state::SessionState::Ended);
            let reason = state.reason.unwrap();
            assert!(reason.contains("已合并"), "{reason}");
            assert!(reason.contains("未核验"), "a bare answer is not §8 evidence: {reason}");
        }
    }

    #[tokio::test]
    async fn m251_open_pr_keeps_session_when_objective_is_retired() {
        for status in ["completed", "legacy_orphan"] {
            let pool = m241_fixture(status, "thin-repair").await;
            sqlx::query("CREATE TABLE IF NOT EXISTS thin_delivery_refs(session_id TEXT PRIMARY KEY,branch TEXT NOT NULL,pr_number INTEGER NOT NULL,ceiling TEXT NOT NULL)").execute(&pool).await.unwrap();
            sqlx::query("INSERT INTO thin_delivery_refs(session_id,branch,pr_number,ceiling) VALUES ('m241','synthetic',1010,'merged')").execute(&pool).await.unwrap();
            assert!(!settle_objectiveless_wake(&pool, "m241", 200).await.unwrap(), "{status}");
            let state = super::super::thin_state::load(&pool, "m241").await.unwrap().unwrap();
            assert_ne!(state.state, super::super::thin_state::SessionState::Ended, "{status}");
            assert_eq!(sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM wakeup_schedules WHERE status='cancelled'").fetch_one(&pool).await.unwrap(), 0);
        }
    }

    #[tokio::test]
    async fn m251_finished_pr_still_ends_a_settled_session() {
        let pool = m241_fixture("completed", "thin-repair").await;
        sqlx::query("CREATE TABLE IF NOT EXISTS thin_delivery_refs(session_id TEXT PRIMARY KEY,branch TEXT NOT NULL,pr_number INTEGER NOT NULL,ceiling TEXT NOT NULL)").execute(&pool).await.unwrap();
        sqlx::query("CREATE TABLE IF NOT EXISTS thin_external_observations(session_id TEXT PRIMARY KEY,pr_number INTEGER NOT NULL,ceiling TEXT NOT NULL,observed_at INTEGER NOT NULL,completed_reason TEXT,closeout_queued INTEGER)").execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO thin_delivery_refs(session_id,branch,pr_number,ceiling) VALUES ('m241','synthetic',1002,'merged')").execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO thin_external_observations(session_id,pr_number,ceiling,observed_at,completed_reason,closeout_queued) VALUES ('m241',1002,'merged',1,'merged',1)").execute(&pool).await.unwrap();
        assert!(settle_objectiveless_wake(&pool, "m241", 200).await.unwrap());
    }

    #[tokio::test]
    async fn m241_unfinished_objective_delivers_normally() {
        let pool = m241_fixture("active", "thin-repair").await;
        let recorder = Arc::new(Recorder::default());
        assert_eq!(drain_due_wakeups_at(&pool, sink(&recorder), 200).await, 1);
        assert_eq!(recorder.take().len(), 1);
        assert_eq!(super::super::thin_state::load(&pool, "m241").await.unwrap().unwrap().state, super::super::thin_state::SessionState::WaitingWake);
    }

    #[tokio::test]
    async fn m241_background_completion_is_cancelled_without_admission() {
        let pool = m241_fixture("completed", "thin-repair").await;
        sqlx::query("INSERT INTO background_tasks(id,session_id,command,cwd,source,status,output_path,created_at,finished_at,timeout_sec,wake_state) VALUES ('m241-bg','m241','synthetic','/synthetic','agent_decided','completed','/synthetic/output',1,100,60,'pending')").execute(&pool).await.unwrap();
        let recorder = Arc::new(Recorder::default());
        assert_eq!(drain_pending_wakes(&pool, sink(&recorder)).await, 0);
        assert!(recorder.take().is_empty());
        assert_eq!(sqlx::query_scalar::<_, String>("SELECT wake_state FROM background_tasks WHERE id='m241-bg'").fetch_one(&pool).await.unwrap(), "cancelled");
        assert_eq!(super::super::thin_state::load(&pool, "m241").await.unwrap().unwrap().state, super::super::thin_state::SessionState::Ended);
    }

    #[tokio::test]
    async fn m241_model_wait_still_resumes_original_objective() {
        let pool = m241_fixture("completed", "model-availability").await;
        sqlx::query("UPDATE wakeup_schedules SET root_turn_id='original'").execute(&pool).await.unwrap();
        let recorder = Arc::new(Recorder::default());
        assert_eq!(drain_due_wakeups_at(&pool, sink(&recorder), 200).await, 1);
        assert_eq!(recorder.take().len(), 1);
        assert_eq!(super::super::thin_state::load(&pool, "m241").await.unwrap().unwrap().state, super::super::thin_state::SessionState::WaitingWake);
    }

    #[tokio::test]
    async fn m241_objective_query_error_does_not_end_session() {
        let pool = m241_fixture("completed", "thin-repair").await;
        sqlx::query("ALTER TABLE objectives RENAME COLUMN status TO broken_status").execute(&pool).await.unwrap();
        let recorder = Arc::new(Recorder::default());
        assert_eq!(drain_due_wakeups_at(&pool, sink(&recorder), 200).await, 0);
        assert!(recorder.take().is_empty());
        assert_eq!(super::super::thin_state::load(&pool, "m241").await.unwrap().unwrap().state, super::super::thin_state::SessionState::WaitingWake);
        assert_eq!(sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM wakeup_schedules WHERE status='pending'").fetch_one(&pool).await.unwrap(), 2);
    }

    #[derive(Default)]
    struct Recorder {
        delivered: StdMutex<Vec<(String, String)>>,
    }

    impl WakeSink for Arc<Recorder> {
        fn enqueue(&self, session_id: String, text: String) -> WakeFuture {
            let me = self.clone();
            Box::pin(async move {
                me.delivered.lock().unwrap().push((session_id, text));
                true
            })
        }
    }

    impl Recorder {
        fn take(&self) -> Vec<(String, String)> {
            std::mem::take(&mut *self.delivered.lock().unwrap())
        }
    }

    fn sink(recorder: &Arc<Recorder>) -> &dyn WakeSink {
        recorder as &dyn WakeSink
    }

    fn alive(pid: u32) -> bool {
        process_alive(pid)
    }

    #[tokio::test]
    async fn a_finished_task_persists_every_field_and_wakes_with_exit_code_and_tail() {
    if crate::agent::trusted_sandbox::sandbox_application_refused() {
        eprintln!("skip: 沙箱内读不到沙箱外进程的启动标记；宿主侧仍全量执行");
        return;
    }
        let pool = memory_pool().await;
        let dir = scratch_dir("finish");
        let started = start(
            &pool,
            request("s1", &dir, "echo bg-tail-ok; echo second-line", None),
        )
        .await
        .unwrap();
        let task = wait_terminal(&pool, &started.id, 30).await;

        assert_eq!(task.status, TaskStatus::Exited);
        assert_eq!(task.exit_code, Some(0));
        assert_eq!(task.session_id, "s1");
        assert_eq!(task.command, "echo bg-tail-ok; echo second-line");
        assert_eq!(task.cwd, dir.to_string_lossy());
        assert!(task.pid.is_some());
        assert!(task.process_started_at.is_some(), "process start marker recorded");
        assert!(Path::new(&task.output_path).exists());
        assert!(task.created_at > 0);
        assert!(task.finished_at.is_some());
        assert_eq!(task.timeout_sec, DEFAULT_TASK_SECS);
        assert_eq!(task.source.as_str(), "agent_decided");

        let recorder = Arc::new(Recorder::default());
        assert_eq!(drain_pending_wakes(&pool, sink(&recorder)).await, 1);
        let delivered = recorder.take();
        assert_eq!(delivered.len(), 1);
        assert_eq!(delivered[0].0, "s1");
        assert!(delivered[0].1.contains("退出码 0"), "{}", delivered[0].1);
        assert!(delivered[0].1.contains("bg-tail-ok"), "{}", delivered[0].1);
        assert!(delivered[0].1.contains(&task.id));
        assert_eq!(
            get(&pool, &started.id).await.unwrap().unwrap().wake_state,
            "enqueued"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn a_failing_command_wakes_with_its_nonzero_exit_code() {
        let pool = memory_pool().await;
        let dir = scratch_dir("fail");
        let started = start(
            &pool,
            request("s1", &dir, "echo boom; exit 7", None),
        )
        .await
        .unwrap();
        let task = wait_terminal(&pool, &started.id, 30).await;
        assert_eq!(task.status, TaskStatus::Failed);
        assert_eq!(task.exit_code, Some(7));

        let recorder = Arc::new(Recorder::default());
        drain_pending_wakes(&pool, sink(&recorder)).await;
        let delivered = recorder.take();
        assert_eq!(delivered.len(), 1);
        assert!(delivered[0].1.contains("失败"), "{}", delivered[0].1);
        assert!(delivered[0].1.contains("7"), "{}", delivered[0].1);
        assert!(delivered[0].1.contains("boom"), "{}", delivered[0].1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn start_returns_immediately_instead_of_waiting_for_the_command() {
        let pool = memory_pool().await;
        let dir = scratch_dir("nowait");
        let begin = std::time::Instant::now();
        let started = start(&pool, request("s1", &dir, "sleep 5", None))
            .await
            .unwrap();
        assert!(
            begin.elapsed() < Duration::from_secs(2),
            "background_task_start blocked for {:?}",
            begin.elapsed()
        );
        assert_eq!(
            get(&pool, &started.id).await.unwrap().unwrap().status,
            TaskStatus::Running
        );
        stop(&pool, "s1", &started.id).await.unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn a_task_past_its_timeout_is_killed_and_marked_timed_out() {
        let pool = memory_pool().await;
        let dir = scratch_dir("timeout");
        let started = start(&pool, request("s1", &dir, "sleep 60", Some(1)))
            .await
            .unwrap();
        let pid = started.pid.expect("pid recorded");
        let task = wait_terminal(&pool, &started.id, 20).await;
        assert_eq!(task.status, TaskStatus::TimedOut);
        assert!(!alive(pid), "timed out process tree must be gone");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn output_tail_keeps_the_end_without_splitting_a_multibyte_character() {
        let dir = scratch_dir("tail");
        let path = dir.join("out.log");
        let body: String = "汉".repeat(5_000);
        std::fs::write(&path, format!("{body}END")).unwrap();
        let tail = output_tail(&path, 101);
        assert!(tail.len() <= 101);
        assert!(tail.ends_with("END"));
        assert!(!tail.contains('\u{FFFD}'));
        assert!(tail.chars().all(|ch| ch == '汉' || ch.is_ascii()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 使用各平台真实 shell，并覆盖含空格、单引号的路径。
    fn grandchild_command(pid_file: &Path) -> String {
        let path = pid_file.to_string_lossy();
        #[cfg(unix)]
        {
            format!("sleep 60 & echo $! > '{}'; wait", path.replace('\'', "'\"'\"'"))
        }
        #[cfg(windows)]
        {
            format!(
                "$p = Start-Process -FilePath powershell -PassThru -ArgumentList '-NoProfile','-Command','Start-Sleep 60'; Set-Content -LiteralPath '{}' -Value $p.Id -Encoding Ascii; Wait-Process -Id $p.Id",
                path.replace('\'', "''")
            )
        }
    }

    #[tokio::test]
    async fn stop_kills_the_whole_process_tree_not_just_the_shell() {
        let pool = memory_pool().await;
        let dir = scratch_dir("tree space's");
        let child_pid_path = dir.join("child.pid");
        let started = start(
            &pool,
            request(
                "s1",
                &dir,
                &grandchild_command(&child_pid_path),
                None,
            ),
        )
        .await
        .unwrap();
        // 等孙进程把 pid 写出来。
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while std::fs::read_to_string(&child_pid_path)
            .map(|text| text.trim().is_empty())
            .unwrap_or(true)
        {
            assert!(std::time::Instant::now() < deadline, "grandchild pid never appeared");
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        let child_pid: u32 = std::fs::read_to_string(&child_pid_path)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        assert!(alive(child_pid));

        let stopped = stop(&pool, "s1", &started.id).await.unwrap();
        assert_eq!(stopped.status, TaskStatus::Stopped);
        assert!(!alive(child_pid), "the grandchild must be killed with the tree");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn a_fourth_task_in_one_session_is_refused_with_a_readable_limit() {
        let pool = memory_pool().await;
        let dir = scratch_dir("per-session");
        let mut ids = Vec::new();
        for _ in 0..MAX_RUNNING_PER_SESSION {
            ids.push(start(&pool, request("s1", &dir, "sleep 60", None)).await.unwrap().id);
        }
        let error = start(&pool, request("s1", &dir, "sleep 60", None))
            .await
            .unwrap_err();
        assert!(error.contains("单会话上限"), "{error}");
        assert!(error.contains(&MAX_RUNNING_PER_SESSION.to_string()), "{error}");
        assert_eq!(list_session(&pool, "s1").await.unwrap().len() as i64, MAX_RUNNING_PER_SESSION);
        reclaim_session(&pool, "s1").await;
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn the_global_limit_refuses_the_task_after_the_cap() {
        let pool = memory_pool().await;
        let dir = scratch_dir("global");
        for index in 0..(MAX_RUNNING_GLOBAL - 1) {
            // 每会话最多 3 个，所以按会话分散启动。
            let session = format!("g{}", index / MAX_RUNNING_PER_SESSION);
            start(&pool, request(&session, &dir, "sleep 60", None))
                .await
                .unwrap();
        }
        let last_session = format!("g{}", (MAX_RUNNING_GLOBAL - 1) / MAX_RUNNING_PER_SESSION);
        start(&pool, request(&last_session, &dir, "sleep 60", None))
            .await
            .unwrap();
        let error = start(&pool, request("overflow", &dir, "sleep 60", None))
            .await
            .unwrap_err();
        assert!(error.contains("全局上限"), "{error}");
        for index in 0..MAX_RUNNING_GLOBAL {
            let session = format!("g{}", index / MAX_RUNNING_PER_SESSION);
            reclaim_session(&pool, &session).await;
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_task_duration_cap_rejects_more_than_three_hours() {
        assert_eq!(validate_timeout(Some(MAX_TASK_SECS)).unwrap(), MAX_TASK_SECS);
        let error = validate_timeout(Some(MAX_TASK_SECS + 1)).unwrap_err();
        assert!(error.contains("3 小时"), "{error}");
        assert!(validate_timeout(Some(0)).is_err());
    }

    #[tokio::test]
    async fn a_command_the_foreground_bash_tool_refuses_is_refused_here_before_any_row_exists() {
        let pool = memory_pool().await;
        let dir = scratch_dir("denied");
        let command = "shutdown /s /t 0";
        assert!(matches!(
            crate::tools::shell_policy::classify_command(command),
            crate::tools::shell_policy::ShellCommandPolicy::Deny { .. }
        ));
        let error = start(&pool, request("s1", &dir, command, None))
            .await
            .unwrap_err();
        assert!(error.contains("被拒绝"), "{error}");
        assert!(list_session(&pool, "s1").await.unwrap().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn list_and_stop_are_scoped_to_the_owning_session() {
        let pool = memory_pool().await;
        let dir = scratch_dir("scope");
        let started = start(&pool, request("owner", &dir, "sleep 60", None))
            .await
            .unwrap();
        assert!(list_session(&pool, "intruder").await.unwrap().is_empty());
        let error = stop(&pool, "intruder", &started.id).await.unwrap_err();
        assert!(error.contains("不属于本会话"), "{error}");
        assert_eq!(list_session(&pool, "owner").await.unwrap().len(), 1);
        reclaim_session(&pool, "owner").await;
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn reconcile_resumes_a_live_task_finishes_a_dead_one_and_marks_unknown_lost() {
    if crate::agent::trusted_sandbox::sandbox_application_refused() {
        eprintln!("skip: 沙箱内读不到沙箱外进程的启动标记；宿主侧仍全量执行");
        return;
    }
        let pool = memory_pool().await;
        let dir = scratch_dir("reconcile");

        // 1) 仍在运行：继续监视。
        let live = start(&pool, request("s1", &dir, "sleep 60", None))
            .await
            .unwrap();

        // 2) 已结束：一个真实存在过、现在已退出的进程 pid。
        let dead_pid = {
            let (program, args) = shell_invocation("exit 0");
            let mut child = std::process::Command::new(program)
                .no_window()
                .args(args)
                .spawn()
                .unwrap();
            let pid = child.id();
            child.wait().unwrap();
            pid
        };
        let dead_output = dir.join("dead.log");
        std::fs::write(&dead_output, "dead-and-gone").unwrap();
        sqlx::query(
            "INSERT INTO background_tasks
                (id, session_id, root_turn_id, command, cwd, source, status, exit_code, pid,
                 process_started_at, output_path, created_at, finished_at, timeout_sec, wake_state)
             VALUES ('bg-dead', 's1', NULL, 'echo dead', ?, 'agent_decided', 'running', NULL, ?,
                     'stale-marker', ?, 1, NULL, 1800, 'none')",
        )
        .bind(dir.to_string_lossy().to_string())
        .bind(dead_pid as i64)
        .bind(dead_output.to_string_lossy().to_string())
        .execute(&pool)
        .await
        .unwrap();

        // 3) 无法确认：没有 pid / 启动时间。
        sqlx::query(
            "INSERT INTO background_tasks
                (id, session_id, root_turn_id, command, cwd, source, status, exit_code, pid,
                 process_started_at, output_path, created_at, finished_at, timeout_sec, wake_state)
             VALUES ('bg-lost', 's2', NULL, 'echo lost', ?, 'agent_decided', 'running', NULL, NULL,
                     NULL, ?, 2, NULL, 1800, 'none')",
        )
        .bind(dir.to_string_lossy().to_string())
        .bind(dir.join("missing.log").to_string_lossy().to_string())
        .execute(&pool)
        .await
        .unwrap();

        let report = reconcile(&pool).await.unwrap();
        assert_eq!(report.resumed, 1, "live task keeps being watched");
        assert_eq!(report.finished, 0, "no exit receipt after restart");
        assert_eq!(report.lost, 2, "dead and unverifiable tasks are lost");

        assert_eq!(
            get(&pool, &live.id).await.unwrap().unwrap().status,
            TaskStatus::Running
        );
        assert_eq!(
            get(&pool, "bg-dead").await.unwrap().unwrap().status,
            TaskStatus::Lost
        );
        assert_eq!(
            get(&pool, "bg-lost").await.unwrap().unwrap().status,
            TaskStatus::Lost
        );

        let recorder = Arc::new(Recorder::default());
        assert_eq!(drain_pending_wakes(&pool, sink(&recorder)).await, 2);
        let delivered = recorder.take();
        assert!(delivered.iter().any(|(_, text)| text.contains("dead-and-gone")));
        assert!(delivered
            .iter()
            .any(|(session, text)| session == "s2" && text.contains("无法确认")));

        // 幂等：再对账一次不会重复补发（live 仍 running，其余已终态且已投递）。
        let second = reconcile(&pool).await.unwrap();
        assert_eq!(second.lost, 0);
        assert_eq!(second.finished, 0);
        assert_eq!(drain_pending_wakes(&pool, sink(&recorder)).await, 0);
        assert!(recorder.take().is_empty());

        stop(&pool, "s1", &live.id).await.unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn reclaiming_a_session_stops_every_task_and_sends_no_wake() {
        let pool = memory_pool().await;
        let dir = scratch_dir("reclaim");
        let first = start(&pool, request("s1", &dir, "sleep 60", None))
            .await
            .unwrap();
        let second = start(&pool, request("s1", &dir, "sleep 60", None))
            .await
            .unwrap();
        let pids: Vec<u32> = [&first, &second]
            .iter()
            .filter_map(|started| started.pid)
            .collect();
        assert_eq!(reclaimed_count(&pool, "s1").await, 2);
        assert_eq!(list_session(&pool, "s1").await.unwrap().len(), 2);
        for task in list_session(&pool, "s1").await.unwrap() {
            assert_eq!(task.status, TaskStatus::Stopped);
            assert_eq!(task.wake_state, "none");
        }
        for pid in pids {
            assert!(!alive(pid), "reclaimed process {pid} must be gone");
        }
        let recorder = Arc::new(Recorder::default());
        assert_eq!(drain_pending_wakes(&pool, sink(&recorder)).await, 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    async fn reclaimed_count(pool: &SqlitePool, session: &str) -> usize {
        reclaim_session(pool, session).await
    }

    // ── 阶段 2：R2 定时唤醒 ────────────────────────────────────────────────

    #[tokio::test]
    async fn external_wait_requires_a_session_owned_wake_source() {
        let pool = memory_pool().await;
        assert!(external_wait_completion_hint(&pool, "s1", false).await.is_none());
        assert!(external_wait_completion_hint(&pool, "s1", true).await.unwrap().contains("schedule_wakeup"));
        schedule_wakeup_at(&pool, wakeup_request("s2", "other", Some(180)), 1000).await.unwrap();
        assert!(external_wait_completion_hint(&pool, "s1", true).await.is_some());
        schedule_wakeup_at(&pool, wakeup_request("s1", "check CI", Some(180)), 1000).await.unwrap();
        assert!(external_wait_completion_hint(&pool, "s1", true).await.is_none());
        reclaim_session(&pool, "s1").await;
        assert!(external_wait_completion_hint(&pool, "s1", true).await.is_some());
    }

    /// 阶段 1 P0-3 / R3：thin 的外部事项等待必须给出两种挂监视的具体做法
    /// （以及 ≥180s 的轮询下限），否则模型只能空转后停在 waiting_system。
    #[tokio::test]
    async fn thin_external_wait_asks_for_a_wake_source_and_waits_when_covered() {
        let pool = memory_pool().await;
        let hint = external_wait_completion_hint(&pool, "thin-session", true)
            .await
            .expect("uncovered external wait must produce a hint");
        assert!(hint.contains("background_task_start"), "{hint}");
        assert!(hint.contains("schedule_wakeup"), "{hint}");
        assert!(hint.contains("180"), "{hint}");
        schedule_wakeup_at(
            &pool,
            wakeup_request("thin-session", "check CI", Some(180)),
            1000,
        )
        .await
        .unwrap();
        assert!(external_wait_completion_hint(&pool, "thin-session", true)
            .await
            .is_none());
    }

    fn wakeup_request(session: &str, prompt: &str, every_sec: Option<u64>) -> WakeupRequest {
        WakeupRequest {
            session_id: session.to_string(),
            root_turn_id: Some("turn-1".to_string()),
            prompt: prompt.to_string(),
            source: TaskSource::AgentDecided,
            after_sec: every_sec.is_none().then_some(60),
            every_sec,
        }
    }

    /// M236/R3：会话里还有新入队的用户消息时，到点的系统唤醒必须让路。
    #[tokio::test]
    async fn a_queued_user_message_defers_a_due_wakeup() {
        let pool = memory_pool().await;
        let request = WakeupRequest {
            after_sec: Some(MIN_WAKEUP_SECS),
            ..wakeup_request("s1", "看一下 CI 状态", None)
        };
        let schedule = schedule_wakeup_at(&pool, request, 1_000).await.unwrap();
        let queued = crate::commands::queued_delivery::enqueue(
            &pool,
            "s1",
            "先按我的方案走",
            Some("turn-user"),
        )
        .await
        .unwrap();

        let recorder = Arc::new(Recorder::default());
        assert_eq!(
            drain_due_wakeups_at(&pool, sink(&recorder), schedule.next_at).await,
            0,
            "the user's queued message outranks the wake-up"
        );
        assert!(recorder.take().is_empty());

        // The wake-up is deferred, not consumed: it stays pending for the next scan.
        let still_pending = get_wakeup(&pool, &schedule.id).await.unwrap().unwrap();
        assert_eq!(still_pending.next_at, schedule.next_at);
        assert_eq!(still_pending.status, "pending");

        // Once the user's message is delivered, the wake-up fires normally.
        crate::commands::queued_delivery::mark_delivered(&pool, "s1", &queued.id)
            .await
            .unwrap();
        assert_eq!(
            drain_due_wakeups_at(&pool, sink(&recorder), schedule.next_at).await,
            1
        );
        assert_eq!(recorder.take().len(), 1);
    }

    #[tokio::test]
    async fn a_due_wakeup_wakes_the_session_and_is_not_fired_early() {
        let pool = memory_pool().await;
        // R2 规格：after_sec / every_sec 二选一必填；一次性唤醒用 after_sec。
        let request = WakeupRequest {
            after_sec: Some(MIN_WAKEUP_SECS),
            ..wakeup_request("s1", "看一下 CI 状态", None)
        };
        let schedule = schedule_wakeup_at(&pool, request, 1_000).await.unwrap();
        assert_eq!(schedule.next_at, 1_060, "one-shot fires after its delay");

        let recorder = Arc::new(Recorder::default());
        assert_eq!(drain_due_wakeups_at(&pool, sink(&recorder), 1_059).await, 0);
        assert!(recorder.take().is_empty());

        assert_eq!(drain_due_wakeups_at(&pool, sink(&recorder), 1_060).await, 1);
        let delivered = recorder.take();
        assert_eq!(delivered.len(), 1);
        assert_eq!(delivered[0].0, "s1");
        assert!(delivered[0].1.contains("定时唤醒"), "{}", delivered[0].1);
        assert!(delivered[0].1.contains(&schedule.id), "{}", delivered[0].1);
        assert!(delivered[0].1.contains("看一下 CI 状态"), "{}", delivered[0].1);

        let stored = list_wakeups(&pool, "s1").await.unwrap();
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].status, "done");
        assert_eq!(stored[0].fire_count, 1);
        // 一次性唤醒不会二次触发。
        assert_eq!(drain_due_wakeups_at(&pool, sink(&recorder), 9_999).await, 0);
    }

    #[tokio::test]
    async fn a_repeating_wakeup_advances_its_schedule_and_fires_again() {
        let pool = memory_pool().await;
        let schedule = schedule_wakeup_at(
            &pool,
            wakeup_request("s1", "每小时检查线上状态", Some(60)),
            1_000,
        )
        .await
        .unwrap();
        assert_eq!(schedule.every_sec, Some(60));
        assert_eq!(schedule.next_at, 1_060);

        let recorder = Arc::new(Recorder::default());
        assert_eq!(drain_due_wakeups_at(&pool, sink(&recorder), 1_060).await, 1);
        let first = list_wakeups(&pool, "s1").await.unwrap();
        assert_eq!(first[0].status, "pending");
        assert_eq!(first[0].fire_count, 1);
        assert_eq!(first[0].next_at, 1_120);
        assert_eq!(first[0].last_fired_at, Some(1_060));

        assert_eq!(drain_due_wakeups_at(&pool, sink(&recorder), 1_119).await, 0);
        assert_eq!(drain_due_wakeups_at(&pool, sink(&recorder), 1_120).await, 1);
        let delivered = recorder.take();
        assert_eq!(delivered.len(), 2);
        assert!(delivered[1].1.contains("第 2 次"), "{}", delivered[1].1);
        let second = list_wakeups(&pool, "s1").await.unwrap();
        assert_eq!(second[0].fire_count, 2);
        assert_eq!(second[0].next_at, 1_180);
    }

    #[tokio::test]
    async fn a_cancelled_wakeup_never_fires_and_cancel_is_idempotent() {
        let pool = memory_pool().await;
        let schedule =
            schedule_wakeup_at(&pool, wakeup_request("s1", "不该触发", Some(60)), 1_000)
                .await
                .unwrap();
        let cancelled = cancel_wakeup(&pool, "s1", &schedule.id).await.unwrap();
        assert_eq!(cancelled.status, "cancelled");
        // 重复取消不报错、状态不变。
        assert_eq!(
            cancel_wakeup(&pool, "s1", &schedule.id).await.unwrap().status,
            "cancelled"
        );

        let recorder = Arc::new(Recorder::default());
        assert_eq!(drain_due_wakeups_at(&pool, sink(&recorder), 5_000).await, 0);
        assert!(recorder.take().is_empty());

        // 别的会话不能取消本会话的唤醒。
        let error = cancel_wakeup(&pool, "intruder", &schedule.id)
            .await
            .unwrap_err();
        assert!(error.contains("不属于本会话"), "{error}");
    }

    async fn file_pool(dir: &Path) -> SqlitePool {
        let url = format!(
            "sqlite://{}?mode=rwc",
            dir.join("db.sqlite").to_string_lossy()
        );
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect(&url)
            .await
            .unwrap();
        ensure_schema(&pool).await.unwrap();
        pool
    }

    #[tokio::test]
    async fn m168_restarted_live_orphan_without_receipt_is_lost() {
        let pool = memory_pool().await;
        let dir = scratch_dir("m168-live-orphan");
        let log = dir.join("output.log");
        std::fs::write(&log, "synthetic output, not an exit receipt").unwrap();
        sqlx::query("INSERT INTO background_tasks(id,session_id,command,cwd,source,status,pid,process_started_at,output_path,created_at,timeout_sec,wake_state) VALUES ('orphan','synthetic','exit 0',?,'agent_decided','running',4294967294,'dead',?,1,60,'none')")
            .bind(dir.to_string_lossy().as_ref()).bind(log.to_string_lossy().as_ref())
            .execute(&pool).await.unwrap();
        let task = get(&pool, "orphan").await.unwrap().unwrap();
        watch_orphan(pool.clone(), task).await;
        let settled = get(&pool, "orphan").await.unwrap().unwrap();
        assert_eq!(settled.status, TaskStatus::Lost);
        assert_eq!(settled.wake_state, "pending");
        let recorder = Arc::new(Recorder::default());
        assert_eq!(drain_pending_wakes(&pool, sink(&recorder)).await, 1);
        assert!(recorder.take()[0].1.contains("无法确认结果"));
        pool.close().await;
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn m168_cross_process_restart_delivers_background_and_window_wakes() {
        const CHILD: &str = "CODEFACTORY_M168_SYNTHETIC_DB";
        if let Ok(path) = std::env::var(CHILD) {
            let pool = file_pool(Path::new(&path)).await;
            reconcile(&pool).await.unwrap();
            let recorder = Arc::new(Recorder::default());
            assert_eq!(drain_pending_wakes(&pool, sink(&recorder)).await, 1);
            assert_eq!(drain_due_wakeups_at(&pool, sink(&recorder), 200).await, 1);
            let delivered = recorder.take();
            assert_eq!(get(&pool, "bg-m168").await.unwrap().unwrap().status, TaskStatus::Lost);
            assert!(delivered.iter().any(|(_, text)| text.contains("无法确认结果")));
            assert!(delivered.iter().any(|(_, text)| text.contains("synthetic window")));
            pool.close().await;
            return;
        }
        let dir = scratch_dir("m168-process-restart");
        let pool = file_pool(&dir).await;
        sqlx::query("INSERT INTO background_tasks(id,session_id,command,cwd,source,status,pid,process_started_at,output_path,created_at,timeout_sec,wake_state) VALUES ('bg-m168','synthetic','exit 0',?,'agent_decided','running',4294967294,'dead',?,1,60,'none')")
            .bind(dir.to_string_lossy().as_ref()).bind(dir.join("existing.log").to_string_lossy().as_ref())
            .execute(&pool).await.unwrap();
        std::fs::write(dir.join("existing.log"), "synthetic output without exit receipt").unwrap();
        schedule_wakeup_at(&pool, WakeupRequest {
            session_id: "synthetic".into(), root_turn_id: None,
            prompt: "synthetic window".into(), source: TaskSource::AgentDecided,
            after_sec: Some(60), every_sec: None,
        }, 100).await.unwrap();
        pool.close().await;
        let output = tokio::process::Command::new(std::env::current_exe().unwrap())
            .args(["m168_cross_process_restart_delivers_background_and_window_wakes", "--nocapture"])
            .env(CHILD, &dir).output().await.unwrap();
        assert!(output.status.success(), "child failed: {} {}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
        let reopened = file_pool(&dir).await;
        assert_eq!(get(&reopened, "bg-m168").await.unwrap().unwrap().wake_state, "enqueued");
        assert_eq!(drain_pending_wakes(&reopened, sink(&Arc::new(Recorder::default()))).await, 0);
        reopened.close().await;
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn m216_triggered_pr_subscription_is_not_a_wake_source() {
        let pool = memory_pool().await;
        sqlx::query("CREATE TABLE thin_external_observations(session_id TEXT PRIMARY KEY,completed_reason TEXT)").execute(&pool).await.unwrap();
        for reason in [Some("PR #42 检查失败：check-rust"), Some("PR #42 被合并队列退回（merge-queue: blocked）"), Some("PR #42 检查已全绿"), Some("PR #42 已合并"), None] {
            sqlx::query("INSERT OR REPLACE INTO thin_external_observations VALUES ('fixture',?)").bind(reason).execute(&pool).await.unwrap();
            assert_eq!(wake_source_cover(&pool, "fixture").await.is_covered(), reason.is_none(), "triggered observation: {reason:?}");
        }
    }

    #[tokio::test]
    async fn m228_busy_is_not_rejection_and_errors_are_redacted() {
        let pool = memory_pool().await;
        super::super::workspace_recovery_budget::ensure(&pool).await.unwrap();
        persist_model_wakeup(&pool,"m228-busy","root",1000,"wait").await.unwrap();
        struct Busy;
        impl WakeSink for Busy {
            fn enqueue(&self, _: String, _: String) -> WakeFuture { Box::pin(async { false }) }
            fn enqueue_detailed(&self, _: String, _: String) -> Pin<Box<dyn std::future::Future<Output=Result<bool,String>> + Send>> { Box::pin(async { Ok(false) }) }
        }
        for n in 0..35 { assert_eq!(drain_due_wakeups_at(&pool,&Busy,100+n*60).await,0); }
        let attempts: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM workspace_recovery_budget WHERE session_id='wake-delivery:m228-busy'").fetch_one(&pool).await.unwrap();
        assert_eq!(attempts,0);
        record_wake_rejection(&pool,"m228-busy","database locked token=syntheticsecret123",3000).await.unwrap();
        let error: String = sqlx::query_scalar("SELECT last_error FROM workspace_recovery_budget WHERE session_id='wake-delivery:m228-busy'").fetch_one(&pool).await.unwrap();
        assert!(error.contains("database locked") && !error.contains("syntheticsecret123"));
        let detail: String = sqlx::query_scalar("SELECT detail_json FROM session_audit_events WHERE session_id='m228-busy'").fetch_one(&pool).await.unwrap();
        assert!(detail.contains("database locked") && !detail.contains("syntheticsecret123"));
    }

    #[tokio::test]
    async fn m228_ended_session_does_not_rearm_startup_interruption() {
        let pool = memory_pool().await;
        super::super::thin_state::ensure(&pool).await.unwrap();
        persist_model_wakeup(&pool, "m228-ended", "root", 1000, "model wait").await.unwrap();
        super::super::thin_state::save(&pool, "m228-ended", &super::super::thin_state::ThinState::ended("PR #42 已合并", "synthetic")).await.unwrap();
        let pending: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM wakeup_schedules WHERE status='pending'").fetch_one(&pool).await.unwrap();
        assert_eq!(pending, 0, "ended must cancel pending wakes");
        persist_model_wakeup(&pool, "m228-ended", "root", 2000, "启动阶段中断").await.unwrap();
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM wakeup_schedules").fetch_one(&pool).await.unwrap();
        assert_eq!(count, 1, "startup recovery must not create another row after settlement");
    }

    #[tokio::test]
    async fn m228_rejected_wakes_explain_and_terminate_after_thirty_attempts() {
        let pool = memory_pool().await;
        super::super::thin_state::ensure(&pool).await.unwrap();
        persist_model_wakeup(&pool,"m228-reject","root",1000,"model reset").await.unwrap();
        super::super::thin_state::save(&pool,"m228-reject", &super::super::thin_state::after_final(super::super::thin_state::WaitNeed::External { what: "model reset".into(), covered: true }, "")).await.unwrap();
        struct Reject;
        impl WakeSink for Reject {
            fn enqueue(&self, _: String, _: String) -> WakeFuture { Box::pin(async { false }) }
        }
        for n in 0..30 {
            assert_eq!(drain_due_wakeups_at(&pool,&Reject,100 + n*60).await,0);
            if n == 4 {
                let state = super::super::thin_state::load(&pool,"m228-reject").await.unwrap().unwrap();
                assert!(state.waiting_for.unwrap_or_default().contains("自动继续失败"));
            }
        }
        let state = super::super::thin_state::load(&pool,"m228-reject").await.unwrap().unwrap();
        assert_eq!(state.state,super::super::thin_state::SessionState::Ended);
        assert!(state.reason.unwrap().contains("30"));
        let audits: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM session_audit_events WHERE session_id='m228-reject' AND kind='wakeup_admission_failed'").fetch_one(&pool).await.unwrap();
        assert_eq!(audits,30);
    }

    #[tokio::test]
    async fn m216_cancelling_last_wake_preserves_wait_atomically() {
        let pool = memory_pool().await;
        super::super::thin_state::ensure(&pool).await.unwrap();
        let wake = schedule_wakeup_at(&pool, wakeup_request("fixture", "PR #42 检查失败", Some(60)), 100).await.unwrap();
        super::super::thin_state::save(&pool, "fixture", &super::super::thin_state::after_final(
            super::super::thin_state::WaitNeed::External { what: "PR #42 修复".into(), covered: true }, "")).await.unwrap();
        cancel_wakeup(&pool, "fixture", &wake.id).await.unwrap();
        assert!(wake_source_cover(&pool, "fixture").await.is_covered(), "cancellation must commit a replacement with waiting state");
        let recorder = Arc::new(Recorder::default());
        assert_eq!(drain_due_wakeups_at(&pool, sink(&recorder), now_secs() + 120).await, 1);
        assert_eq!(recorder.take().len(), 1);
    }

    #[tokio::test]
    async fn m216_stopping_wait_cancels_without_replacement() {
        let pool = memory_pool().await;
        super::super::thin_state::ensure(&pool).await.unwrap();
        schedule_wakeup_at(&pool, wakeup_request("fixture", "PR #42 检查失败", Some(60)), 100).await.unwrap();
        super::super::thin_state::save(&pool, "fixture", &super::super::thin_state::after_final(
            super::super::thin_state::WaitNeed::External { what: "PR #42 修复".into(), covered: true }, "")).await.unwrap();
        assert_eq!(cancel_session_wakeups(&pool, "fixture").await, 1);
        assert_eq!(super::super::thin_state::load(&pool, "fixture").await.unwrap().unwrap().state, super::super::thin_state::SessionState::Ended);
        assert!(!wake_source_cover(&pool, "fixture").await.is_covered());
        let recorder = Arc::new(Recorder::default());
        assert_eq!(drain_due_wakeups_at(&pool, sink(&recorder), now_secs() + 120).await, 0);
        assert!(recorder.take().is_empty());
    }

    #[tokio::test]
    async fn m216_ci_then_blocked_survives_delivery_failures() {
        let pool = crate::storage::db::connect("sqlite::memory:").await.unwrap();
        ensure_schema(&pool).await.unwrap();
        super::super::workspace_recovery_budget::ensure(&pool).await.unwrap();
        sqlx::query("INSERT INTO sessions(id,title,cwd,model_id,created_at,updated_at,delivery_mode) VALUES ('fixture','fixture','/synthetic','model',1,1,'thin')").execute(&pool).await.unwrap();
        // A previously answered task: M209's unanswered-user escape must not hide the defect.
        for (id, prompt, at) in [("thin-repair-ci", "PR #42 检查失败：check-rust", 100), ("thin-repair-blocked", "PR #42 被合并队列退回（merge-queue: blocked）", 110)] {
            sqlx::query("INSERT INTO wakeup_schedules(id,session_id,prompt,source,next_at,status,created_at) VALUES (?,'fixture',?,'agent_decided',?,'pending',1)")
                .bind(id).bind(prompt).bind(at).execute(&pool).await.unwrap();
        }
        struct Busy;
        impl WakeSink for Busy {
            fn enqueue(&self, _: String, _: String) -> WakeFuture { Box::pin(async { false }) }
        }
        let at = now_secs();
        for offset in [0, 30, 60] { assert_eq!(drain_due_wakeups_at(&pool, &Busy, at + offset).await, 0); }
        let cancelled: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM wakeup_schedules WHERE status='cancelled'").fetch_one(&pool).await.unwrap();
        assert_eq!(cancelled, 0, "technical admission failures must not cancel repair wakes");
        let recorder = Arc::new(Recorder::default());
        assert_eq!(drain_due_wakeups_at(&pool, sink(&recorder), at + 120).await, 1);
        let calls = recorder.take();
        assert_eq!(calls.len(), 1);
        assert!(calls[0].1.contains("check-rust") && calls[0].1.contains("merge-queue: blocked"));
        let fired: i64 = sqlx::query_scalar("SELECT SUM(fire_count) FROM wakeup_schedules WHERE status='done'").fetch_one(&pool).await.unwrap();
        assert_eq!(fired, 2);
    }

    #[tokio::test]
    async fn m209_reused_pending_repair_ignores_stale_workspace_budget() {
        let pool = crate::storage::db::connect("sqlite::memory:").await.unwrap();
        ensure_schema(&pool).await.unwrap();
        super::super::workspace_recovery_budget::ensure(&pool).await.unwrap();
        sqlx::raw_sql("INSERT INTO sessions(id,title,cwd,model_id,created_at,updated_at,delivery_mode) VALUES ('synthetic-reopen','fixture','/synthetic','model',1,1,'thin');
            INSERT INTO messages(id,session_id,role,content,created_at) VALUES ('new-intent','synthetic-reopen','user','continue interrupted task',200);
            INSERT INTO workspace_recovery_budget(session_id,attempts,next_at,settled_reason) VALUES ('synthetic-reopen',3,999999,'old task settled');").execute(&pool).await.unwrap();
        for count in [0_i64, 1, 4] {
            sqlx::query("INSERT INTO wakeup_schedules(id,session_id,prompt,source,next_at,status,fire_count,created_at) VALUES (?,'synthetic-reopen','resume','agent_decided',100,'pending',?,1)")
                .bind(format!("thin-source-repair-synthetic-{count}")).bind(count).execute(&pool).await.unwrap();
        }
        let recorder = Arc::new(Recorder::default());
        assert_eq!(drain_due_wakeups_at(&pool, sink(&recorder), 20000).await, 1);
        assert_eq!(recorder.take().len(), 1);
        let pending: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM wakeup_schedules WHERE status='pending' AND next_at<20000-120").fetch_one(&pool).await.unwrap();
        assert_eq!(pending, 0, "all fire counts must be handled in one scan");
        let counts: Vec<i64> = sqlx::query_scalar("SELECT fire_count FROM wakeup_schedules ORDER BY fire_count").fetch_all(&pool).await.unwrap();
        assert_eq!(counts, vec![1,2,5]);
    }

    #[tokio::test]
    async fn m237_due_wakes_hold_until_the_model_wake_without_writing_messages() {
        let dir = scratch_dir("m237-hold");
        let pool = file_pool(&dir).await;
        super::super::thin_state::ensure(&pool).await.unwrap();
        super::super::workspace_recovery_budget::ensure(&pool).await.unwrap();
        sqlx::raw_sql("CREATE TABLE sessions(id TEXT PRIMARY KEY,delivery_mode TEXT);
            CREATE TABLE thin_external_observations(session_id TEXT PRIMARY KEY,completed_reason TEXT);
            CREATE TABLE session_audit_events(id TEXT PRIMARY KEY,session_id TEXT,kind TEXT,actor TEXT,detail_json TEXT,created_at INTEGER);
            INSERT INTO sessions VALUES ('m237','thin');").execute(&pool).await.unwrap();
        ensure_wakeup_schema(&pool).await.unwrap();
        for (id, at) in [("thin-closeout-m237-42", 100), ("model-availability-m237", 5_000)] {
            sqlx::query("INSERT INTO wakeup_schedules(id,session_id,prompt,source,after_sec,next_at,status,created_at) VALUES (?,'m237','PR #42 已合并，执行收尾','agent_decided',60,?,'pending',1)")
                .bind(id).bind(at).execute(&pool).await.unwrap();
        }
        let recorder = Arc::new(Recorder::default());
        for now in (200..4_000).step_by(60) {
            assert_eq!(drain_due_wakeups_at(&pool, sink(&recorder), now).await, 0);
        }
        assert!(recorder.take().is_empty(), "no model means no wake text in the transcript");
        let held: (i64, i64) = sqlx::query_as("SELECT fire_count,next_at FROM wakeup_schedules WHERE id='thin-closeout-m237-42'")
            .fetch_one(&pool).await.unwrap();
        assert_eq!(held, (0, 5_000), "a held wake is deferred to the model wake, not fired or left overdue");
        assert_eq!(drain_due_wakeups_at(&pool, sink(&recorder), 5_000).await, 1);
        assert_eq!(recorder.take().len(), 1, "wakes held for one session resume as one turn");
        pool.close().await;
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn m205_restart_backlog_never_silently_skips_exhausted_recovery() {
        let dir = scratch_dir("m205-backlog");
        let pool = file_pool(&dir).await;
        super::super::thin_state::ensure(&pool).await.unwrap();
        super::super::workspace_recovery_budget::ensure(&pool).await.unwrap();
        sqlx::raw_sql("CREATE TABLE sessions(id TEXT PRIMARY KEY,delivery_mode TEXT);
            CREATE TABLE thin_external_observations(session_id TEXT PRIMARY KEY,completed_reason TEXT);
            CREATE TABLE session_audit_events(id TEXT PRIMARY KEY,session_id TEXT,kind TEXT,actor TEXT,detail_json TEXT,created_at INTEGER);").execute(&pool).await.unwrap();
        for index in 0..5 {
            let session = format!("synthetic-backlog-{index}");
            sqlx::query("INSERT INTO sessions VALUES (?,'thin')").bind(&session).execute(&pool).await.unwrap();
            sqlx::query("INSERT INTO thin_external_observations VALUES (?,'PR #42 已合并')").bind(&session).execute(&pool).await.unwrap();
            // M231: legacy restart fixture deliberately bypasses new-wait admission.
            sqlx::query("INSERT INTO thin_session_state(session_id,state,waiting_for,updated_at) VALUES (?,'waiting_wake','PR #42 收尾',0)")
                .bind(&session).execute(&pool).await.unwrap();
            sqlx::query("INSERT INTO workspace_recovery_budget(session_id,attempts,next_at,settled_reason) VALUES (?,?,0,?)")
                .bind(&session).bind(if index == 0 { 0 } else { 3 })
                .bind(if index == 4 { Some("登记工作区已删除") } else { None }).execute(&pool).await.unwrap();
            let prefix = if index < 3 { "thin-closeout" } else { "thin-repair" };
            sqlx::query("INSERT INTO wakeup_schedules(id,session_id,prompt,source,next_at,status,created_at) VALUES (?,?, 'PR #42 已合并，执行收尾','agent_decided',100,'pending',1)")
                .bind(format!("{prefix}-{index}")).bind(&session).execute(&pool).await.unwrap();
        }
        pool.close().await;
        let pool = file_pool(&dir).await;
        let recorder = Arc::new(Recorder::default());
        drain_due_wakeups_at(&pool, sink(&recorder), 20_000).await;
        let pending: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM wakeup_schedules WHERE status='pending' AND next_at < ?-120")
            .bind(20_000_i64).fetch_one(&pool).await.unwrap();
        assert_eq!(pending, 0, "one startup scan must dispose all overdue closeout/repair wakes, including exhausted budgets");
        let audit: i64 = sqlx::query_scalar("SELECT COUNT(DISTINCT session_id) FROM session_audit_events WHERE kind IN ('wakeup_overdue_recovered','wakeup_rejected')")
            .fetch_one(&pool).await.unwrap();
        assert_eq!(audit, 5, "every overdue wake must have a durable delivery or rejection receipt");
        for index in 1..5 {
            let state = super::super::thin_state::load(&pool, &format!("synthetic-backlog-{index}")).await.unwrap().unwrap();
            assert_eq!(state.state, super::super::thin_state::SessionState::Ended);
            assert!(state.reason.as_deref().is_some_and(|reason| reason.contains("PR #42") && reason.contains("恢复")));
        }
        assert_eq!(recorder.take().len(), 1);
        assert_eq!(drain_due_wakeups_at(&pool, sink(&recorder), 20_001).await, 0);
        pool.close().await;
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn m202_overdue_wake_is_audited_and_delivered() {
        let dir = scratch_dir("m202-overdue");
        let pool = file_pool(&dir).await;
        sqlx::query("CREATE TABLE IF NOT EXISTS session_audit_events(id TEXT PRIMARY KEY,session_id TEXT,kind TEXT,actor TEXT,detail_json TEXT,created_at INTEGER)").execute(&pool).await.unwrap();
        schedule_wakeup_at(&pool, wakeup_request("s1", "synthetic overdue", Some(180)), 1000).await.unwrap();
        let recorder = Arc::new(Recorder::default());
        assert_eq!(drain_due_wakeups_at(&pool, sink(&recorder), 1600).await, 1);
        assert_eq!(recorder.take().len(), 1);
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM session_audit_events WHERE session_id='s1' AND kind='wakeup_overdue_recovered'").fetch_one(&pool).await.unwrap();
        assert_eq!(count, 1, "overdue recovery must be durable audit evidence");
        assert_eq!(drain_due_wakeups_at(&pool, sink(&recorder), 1600).await, 0);
        pool.close().await;
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn a_wakeup_survives_a_restart_and_catches_up_only_once() {
        let dir = scratch_dir("wakeup-restart");
        let pool = file_pool(&dir).await;
        let schedule = schedule_wakeup_at(
            &pool,
            wakeup_request("s1", "重启后继续巡检", Some(300)),
            1_000,
        )
        .await
        .unwrap();
        // 应用关闭：连接关掉。
        pool.close().await;

        // 重启：重新打开同一个库，计划仍在。
        let reopened = file_pool(&dir).await;
        let stored = list_wakeups(&reopened, "s1").await.unwrap();
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].id, schedule.id);
        assert_eq!(stored[0].status, "pending");

        // 关闭期间错过 3 个周期：启动后只补一次，然后按周期继续。
        let late = 1_000 + 300 * 3;
        let recorder = Arc::new(Recorder::default());
        assert_eq!(drain_due_wakeups_at(&reopened, sink(&recorder), late).await, 1);
        let delivered = recorder.take();
        assert_eq!(delivered.len(), 1);
        assert!(
            delivered[0].1.contains("补发"),
            "catch-up wake must say the plan was missed: {}",
            delivered[0].1
        );
        let after = list_wakeups(&reopened, "s1").await.unwrap();
        assert_eq!(after[0].fire_count, 1, "a missed schedule catches up once");
        assert_eq!(after[0].next_at, late + 300);

        // 同一次到点不会被重复投递（幂等）。
        assert_eq!(drain_due_wakeups_at(&reopened, sink(&recorder), late).await, 0);
        reopened.close().await;
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn wakeup_delays_outside_the_bounds_are_refused_with_a_readable_error() {
        assert_eq!(MIN_WAKEUP_SECS, 60);
        assert_eq!(MAX_WAKEUP_SECS, 21_600);
        for value in [MIN_WAKEUP_SECS, MAX_WAKEUP_SECS] {
            validate_wakeup_delay(None, Some(value)).unwrap();
            validate_wakeup_delay(Some(value), None).unwrap();
        }
        let too_soon = validate_wakeup_delay(None, Some(MIN_WAKEUP_SECS - 1)).unwrap_err();
        assert!(too_soon.contains("下限"), "{too_soon}");
        assert!(too_soon.contains("60"), "{too_soon}");
        let too_late = validate_wakeup_delay(Some(MAX_WAKEUP_SECS + 1), None).unwrap_err();
        assert!(too_late.contains("上限"), "{too_late}");
        assert!(too_late.contains("6 小时"), "{too_late}");
        assert!(validate_wakeup_delay(None, Some(0)).is_err());
        assert!(validate_wakeup_delay(None, None).is_err());
        assert!(validate_wakeup_delay(Some(600), Some(600)).is_err());
    }

    #[test]
    fn m149_zero_valued_wakeup_field_counts_as_not_provided() {
        // M149 R1：模型习惯一次把两个字段都写上，不用的那个写 0。0 必须等同于「没传」，
        // 否则一次性的等待会被判成「二选一」冲突，本回合就没有任何唤醒源。
        assert_eq!(
            validate_wakeup_delay(Some(180), Some(0)).unwrap(),
            (Some(180), None)
        );
        assert_eq!(
            validate_wakeup_delay(Some(0), Some(600)).unwrap(),
            (None, Some(600))
        );
        let nothing_usable = validate_wakeup_delay(None, Some(0)).unwrap_err();
        assert!(nothing_usable.contains("after_sec"), "{nothing_usable}");
        assert!(nothing_usable.contains("180"), "{nothing_usable}");
        let conflict = validate_wakeup_delay(Some(180), Some(600)).unwrap_err();
        assert!(conflict.contains("\"after_sec\":180"), "{conflict}");
        assert!(conflict.contains("0"), "{conflict}");
    }

    #[tokio::test]
    async fn the_wakeup_cap_refuses_the_ninth_with_a_readable_limit() {
        let pool = memory_pool().await;
        for _ in 0..MAX_WAKEUPS_PER_SESSION {
            schedule_wakeup_at(&pool, wakeup_request("s1", "巡检", Some(60)), 1_000)
                .await
                .unwrap();
        }
        let error = schedule_wakeup_at(&pool, wakeup_request("s1", "巡检", Some(60)), 1_000)
            .await
            .unwrap_err();
        assert!(error.contains("上限"), "{error}");
        assert!(
            error.contains(&MAX_WAKEUPS_PER_SESSION.to_string()),
            "{error}"
        );
        assert!(error.contains("wakeup_cancel"), "{error}");
        assert_eq!(
            list_wakeups(&pool, "s1").await.unwrap().len() as i64,
            MAX_WAKEUPS_PER_SESSION
        );
    }

    #[tokio::test]
    async fn reclaiming_a_session_cancels_its_wakeups_and_keeps_other_sessions() {
        let pool = memory_pool().await;
        schedule_wakeup_at(&pool, wakeup_request("s1", "巡检一", Some(60)), 1_000)
            .await
            .unwrap();
        schedule_wakeup_at(&pool, wakeup_request("s1", "巡检二", Some(60)), 1_000)
            .await
            .unwrap();
        schedule_wakeup_at(&pool, wakeup_request("s2", "别人还在用", Some(60)), 1_000)
            .await
            .unwrap();

        assert_eq!(cancel_session_wakeups(&pool, "s1").await, 2);
        assert!(list_wakeups(&pool, "s1")
            .await
            .unwrap()
            .iter()
            .all(|wakeup| wakeup.status == "cancelled"));

        let recorder = Arc::new(Recorder::default());
        assert_eq!(drain_due_wakeups_at(&pool, sink(&recorder), 5_000).await, 1);
        let delivered = recorder.take();
        assert_eq!(delivered.len(), 1);
        assert_eq!(delivered[0].0, "s2");
    }

    #[tokio::test]
    async fn both_task_sources_are_recorded_and_visible() {
        let pool = memory_pool().await;
        let dir = scratch_dir("sources");
        let user_asked = start(
            &pool,
            StartRequest {
                source: TaskSource::UserRequested,
                ..request("s1", &dir, "sleep 60", None)
            },
        )
        .await
        .unwrap();
        let agent_chose = start(&pool, request("s1", &dir, "sleep 60", None))
            .await
            .unwrap();
        let tasks = list_session(&pool, "s1").await.unwrap();
        let by_id = |id: &str| tasks.iter().find(|task| task.id == id).cloned().unwrap();
        assert_eq!(
            by_id(&user_asked.id).source,
            TaskSource::UserRequested,
            "a user-requested task records its source"
        );
        assert_eq!(by_id(&agent_chose.id).source, TaskSource::AgentDecided);

        let mut wakeup = wakeup_request("s1", "盯着 PR，合并了告诉我", Some(60));
        wakeup.source = TaskSource::UserRequested;
        let scheduled = schedule_wakeup_at(&pool, wakeup, 1_000).await.unwrap();
        assert_eq!(scheduled.source, TaskSource::UserRequested);

        reclaim_session(&pool, "s1").await;
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn the_completion_gate_sees_a_running_task_or_a_pending_wakeup_as_a_wake_source() {
        let pool = memory_pool().await;
        let dir = scratch_dir("cover");

        let empty = wake_source_cover(&pool, "s1").await;
        assert!(!empty.is_covered());
        let hint = empty.missing_hint().expect("missing hint");
        assert!(hint.contains("background_task_start"), "{hint}");
        assert!(hint.contains("schedule_wakeup"), "{hint}");

        let started = start(&pool, request("s1", &dir, "sleep 60", None))
            .await
            .unwrap();
        let with_task = wake_source_cover(&pool, "s1").await;
        assert!(with_task.is_covered());
        assert_eq!(with_task.running_tasks, 1);
        assert!(with_task.missing_hint().is_none());

        stop(&pool, "s1", &started.id).await.unwrap();
        assert!(!wake_source_cover(&pool, "s1").await.is_covered());

        schedule_wakeup_at(&pool, wakeup_request("s1", "巡检", Some(60)), 1_000)
            .await
            .unwrap();
        let with_wakeup = wake_source_cover(&pool, "s1").await;
        assert!(with_wakeup.is_covered());
        assert_eq!(with_wakeup.pending_wakeups, 1);

        // 别的会话的唤醒不算本会话的唤醒源。
        assert!(!wake_source_cover(&pool, "s2").await.is_covered());
        cancel_session_wakeups(&pool, "s1").await;
        let _ = std::fs::remove_dir_all(&dir);
    }
}

