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
    if bytes.len() <= limit {
        return String::from_utf8_lossy(&bytes).into_owned();
    }
    let mut start = bytes.len().saturating_sub(limit);
    while start < bytes.len() && !is_char_boundary(&bytes, start) {
        start += 1;
    }
    String::from_utf8_lossy(&bytes[start..]).into_owned()
}

fn is_char_boundary(bytes: &[u8], index: usize) -> bool {
    index == bytes.len() || (bytes[index] & 0xC0) != 0x80
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
    (
        "powershell".to_string(),
        vec![
            "-NoProfile".to_string(),
            "-ExecutionPolicy".to_string(),
            "Bypass".to_string(),
            "-Command".to_string(),
            command.to_string(),
        ],
    )
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
) {
    let deadline = tokio::time::Instant::now() + timeout;
    match tokio::time::timeout_at(deadline, child.wait()).await {
        Ok(Ok(status)) => {
            let code = status.code();
            let terminal = if code == Some(0) {
                TaskStatus::Exited
            } else {
                TaskStatus::Failed
            };
            settle(&pool, &id, terminal, code, true).await;
        }
        Ok(Err(_)) => {
            settle(&pool, &id, TaskStatus::Failed, None, true).await;
        }
        Err(_) => {
            if let Some(pid) = pid {
                terminate_tree(pid).await;
            }
            let _ = tokio::time::timeout(Duration::from_secs(5), child.wait()).await;
            settle(&pool, &id, TaskStatus::TimedOut, None, true).await;
        }
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
                let output_present = Path::new(&task.output_path).exists();
                let status = if output_present {
                    TaskStatus::Exited
                } else {
                    TaskStatus::Lost
                };
                if settle(pool, &task.id, status, None, true).await {
                    report.finished += 1;
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
            let present = Path::new(&task.output_path).exists();
            let status = if present {
                TaskStatus::Exited
            } else {
                TaskStatus::Lost
            };
            settle(&pool, &task.id, status, None, true).await;
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
}

/// D2：把待投递的唤醒交给会话入队，投递成功才标记 `enqueued`，因此重启后不会重复补发。
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
    let mut delivered = 0;
    for row in rows {
        let task = row_to_task(&row);
        let text = wake_text(&task);
        if sink.enqueue(task.session_id.clone(), text).await {
            let _ = sqlx::query(
                "UPDATE background_tasks SET wake_state = 'enqueued' WHERE id = ? AND wake_state = 'pending'",
            )
            .bind(&task.id)
            .execute(pool)
            .await;
            delivered += 1;
        }
    }
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
    if after_sec.is_some() && every_sec.is_some() {
        return Err(
            "after_sec 与 every_sec 只能二选一：要一次性唤醒用 after_sec，要周期性唤醒用 every_sec。"
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
                 （每 N 秒唤醒一次），范围 {MIN_WAKEUP_SECS}–{MAX_WAKEUP_SECS} 秒。"
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
    Ok(())
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
    sqlx::query("UPDATE wakeup_schedules SET status = 'cancelled' WHERE id = ? AND status = 'pending'")
        .bind(id)
        .execute(pool)
        .await
        .map_err(|error| error.to_string())?;
    tracing::info!(wakeup = %id, session = %session_id, "wakeup cancelled");
    get_wakeup(pool, id)
        .await?
        .ok_or_else(|| format!("定时唤醒 {id} 在取消后消失。"))
}

/// R2：会话停止/结案时取消其全部待触发唤醒（与后台任务回收同一入口）。
pub async fn cancel_session_wakeups(pool: &SqlitePool, session_id: &str) -> usize {
    if ensure_wakeup_schema(pool).await.is_err() {
        return 0;
    }
    let result = sqlx::query(
        "UPDATE wakeup_schedules SET status = 'cancelled'
          WHERE session_id = ? AND status = 'pending'",
    )
    .bind(session_id)
    .execute(pool)
    .await;
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
pub async fn drain_due_wakeups_at(pool: &SqlitePool, sink: &dyn WakeSink, now: i64) -> usize {
    if ensure_wakeup_schema(pool).await.is_err() {
        return 0;
    }
    let rows = sqlx::query(
        "SELECT * FROM wakeup_schedules WHERE status = 'pending' AND next_at <= ?
          ORDER BY next_at, id LIMIT 32",
    )
    .bind(now)
    .fetch_all(pool)
    .await
    .unwrap_or_default();
    let mut delivered = 0;
    for row in rows {
        let wakeup = row_to_wakeup(&row);
        // 关闭期间错过的周期只补一次：只要晚于一个周期就算迟到，文本里说明。
        let grace = wakeup.every_sec.unwrap_or(1).max(1) as i64;
        let late = now - wakeup.next_at >= grace;
        let text = wakeup_text(&wakeup, wakeup.fire_count + 1, late);
        if !sink.enqueue(wakeup.session_id.clone(), text).await {
            continue;
        }
        let advanced = match wakeup.every_sec {
            Some(every) => sqlx::query(
                "UPDATE wakeup_schedules
                    SET next_at = ?, fire_count = fire_count + 1, last_fired_at = ?
                  WHERE id = ? AND status = 'pending' AND next_at = ?",
            )
            .bind(now + every as i64)
            .bind(now)
            .bind(&wakeup.id)
            .bind(wakeup.next_at)
            .execute(pool)
            .await
            .map(|done| done.rows_affected() == 1),
            None => sqlx::query(
                "UPDATE wakeup_schedules
                    SET status = 'done', fire_count = fire_count + 1, last_fired_at = ?
                  WHERE id = ? AND status = 'pending' AND next_at = ?",
            )
            .bind(now)
            .bind(&wakeup.id)
            .bind(wakeup.next_at)
            .execute(pool)
            .await
            .map(|done| done.rows_affected() == 1),
        };
        if matches!(advanced, Ok(true)) {
            delivered += 1;
            tracing::info!(
                wakeup = %wakeup.id,
                session = %wakeup.session_id,
                occurrence = wakeup.fire_count + 1,
                late,
                "wakeup fired"
            );
        }
    }
    delivered
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
}

impl WakeSourceCover {
    pub fn is_covered(&self) -> bool {
        self.running_tasks + self.pending_wakeups > 0
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
    WakeSourceCover {
        running_tasks: running_tasks.max(0) as usize,
        pending_wakeups: pending_wakeups.max(0) as usize,
    }
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
        match reconcile(&pool).await {
            Ok(report) => tracing::info!(
                resumed = report.resumed,
                finished = report.finished,
                lost = report.lost,
                "background task reconcile finished"
            ),
            Err(error) => tracing::warn!(%error, "background task reconcile failed"),
        }
        let sink = ChatWakeSink { app: app.clone() };
        loop {
            let delivered = drain_pending_wakes(&pool, &sink).await;
            if delivered > 0 {
                tracing::info!(delivered, "background task wakes enqueued");
            }
            // R2：每秒检查一次到点唤醒（重启后错过的在启动首轮补发一次）。
            let fired = drain_due_wakeups(&pool, &sink).await;
            if fired > 0 {
                tracing::info!(fired, "scheduled wakeups enqueued");
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
        let app = self.app.clone();
        Box::pin(async move {
            use tauri::Manager;
            let state = app.state::<crate::AppState>();
            let mcp = app.state::<std::sync::Arc<crate::mcp::McpManager>>();
            // 与用户输入同一条入队通道：会话正在跑回合时 send_message 会拒绝，
            // 该 wake 保持 pending，下一轮循环重试（不会丢）。
            match crate::commands::chat::send_message(app.clone(), session_id, text, None, state, mcp)
                .await
            {
                Ok(()) => true,
                Err(error) => {
                    tracing::debug!(%error, "background task wake deferred");
                    false
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex as StdMutex};

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
        assert_eq!(report.finished, 1, "dead task gets its terminal state");
        assert_eq!(report.lost, 1, "unverifiable task is marked lost");

        assert_eq!(
            get(&pool, &live.id).await.unwrap().unwrap().status,
            TaskStatus::Running
        );
        assert_eq!(
            get(&pool, "bg-dead").await.unwrap().unwrap().status,
            TaskStatus::Exited
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

    fn wakeup_request(session: &str, prompt: &str, every_sec: Option<u64>) -> WakeupRequest {
        WakeupRequest {
            session_id: session.to_string(),
            root_turn_id: Some("turn-1".to_string()),
            prompt: prompt.to_string(),
            source: TaskSource::AgentDecided,
            after_sec: None,
            every_sec,
        }
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

