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

/// R3：会话被停止/结案时回收全部后台任务，不再发唤醒。
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
            tokio::time::sleep(Duration::from_secs(2)).await;
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
}

