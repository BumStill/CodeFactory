// SPDX-License-Identifier: Apache-2.0
//! 会话内命令活动登记，以及"可中断、能识别等待输入"的流式命令执行。
//!
//! `util::process_tree` 是无人值守 smoke 的受信驱动文件（其摘要被 execute gate
//! 钉在默认分支上，普通 PR 不得改动），因此这里只使用它的既有公开面
//! （`isolate_std_process_tree` 与 `StdProcessTree`），把活动登记、提示符识别
//! 与停止信号放在普通模块里实现，既满足 M143 的产品行为，也不改写受信驱动。

use std::collections::HashMap;
use std::process::{Output, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use tokio::io::AsyncReadExt;
use tokio::process::{Child, Command};

/// 命令结束原因：前三项与 `process_tree::ProcessOutputError` 对齐，
/// 后两项是 M143 新增的"等待用户输入 / 被用户停止"。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandActivityFailure {
    Timeout,
    Unavailable,
    Failed,
    InputRequired,
    Stopped,
}

#[derive(Clone, Debug, serde::Serialize)]
pub struct CommandActivity {
    pub id: String,
    pub session_id: String,
    pub command: String,
    pub last_output_at: u64,
}

type Registry = Mutex<HashMap<String, (CommandActivity, Arc<AtomicBool>)>>;

fn activities() -> &'static Registry {
    static ITEMS: OnceLock<Registry> = OnceLock::new();
    ITEMS.get_or_init(Default::default)
}

fn now_sec() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

pub struct ActivityGuard {
    pub id: String,
    cancelled: Arc<AtomicBool>,
}

impl ActivityGuard {
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Relaxed)
    }
}

impl Drop for ActivityGuard {
    fn drop(&mut self) {
        activities().lock().unwrap().remove(&self.id);
    }
}

pub fn track_command(session_id: &str, command: &str) -> ActivityGuard {
    let id = uuid::Uuid::new_v4().to_string();
    let cancelled = Arc::new(AtomicBool::new(false));
    activities().lock().unwrap().insert(
        id.clone(),
        (
            CommandActivity {
                id: id.clone(),
                session_id: session_id.into(),
                command: command.into(),
                last_output_at: now_sec(),
            },
            cancelled.clone(),
        ),
    );
    ActivityGuard { id, cancelled }
}

pub fn command_activity(session_id: &str) -> Vec<CommandActivity> {
    activities()
        .lock()
        .unwrap()
        .values()
        .filter(|(activity, _)| activity.session_id == session_id)
        .map(|(activity, _)| activity.clone())
        .collect()
}

pub fn stop_command(session_id: &str, id: &str) -> bool {
    let items = activities().lock().unwrap();
    if let Some((activity, stop)) = items.get(id) {
        if activity.session_id == session_id {
            stop.store(true, Ordering::Relaxed);
            return true;
        }
    }
    false
}

fn mark_output(id: Option<&str>) {
    if let Some(id) = id {
        if let Some((item, _)) = activities().lock().unwrap().get_mut(id) {
            item.last_output_at = now_sec();
        }
    }
}

/// Job Object 守卫：Windows 上关闭句柄即杀掉整棵进程树，即使父进程已退出。
#[cfg(windows)]
pub(crate) struct JobGuard(windows_sys::Win32::Foundation::HANDLE);

#[cfg(windows)]
impl Drop for JobGuard {
    fn drop(&mut self) {
        unsafe { windows_sys::Win32::Foundation::CloseHandle(self.0) };
    }
}

#[cfg(windows)]
pub(crate) fn attach_job(
    child: std::os::windows::io::RawHandle,
) -> std::io::Result<JobGuard> {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
        SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    };

    let job = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
    if job.is_null() {
        return Err(std::io::Error::last_os_error());
    }
    let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
    limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
    let configured = unsafe {
        SetInformationJobObject(
            job,
            JobObjectExtendedLimitInformation,
            &limits as *const _ as *const std::ffi::c_void,
            std::mem::size_of_val(&limits) as u32,
        )
    };
    let assigned = configured != 0
        && unsafe { AssignProcessToJobObject(job, child as _) } != 0;
    if !assigned {
        let error = std::io::Error::last_os_error();
        unsafe { CloseHandle(job) };
        return Err(error);
    }
    Ok(JobGuard(job))
}

async fn capture_stream<R: tokio::io::AsyncRead + Unpin>(
    mut reader: R,
    id: Option<String>,
    prompt: Arc<AtomicBool>,
    sink: Arc<std::sync::Mutex<Vec<u8>>>,
) -> std::io::Result<()> {
    let mut chunk = [0u8; 4096];
    loop {
        let count = reader.read(&mut chunk).await?;
        if count == 0 {
            return Ok(());
        }
        let tail_start = {
            let mut bytes = sink.lock().expect("capture sink poisoned");
            let start = bytes.len().saturating_sub(4096);
            bytes.extend_from_slice(&chunk[..count]);
            start
        };
        mark_output(id.as_deref());
        let tail = {
            let bytes = sink.lock().expect("capture sink poisoned");
            crate::util::command_env::decode_output(&bytes[tail_start..])
        };
        if crate::util::command_env::waiting_for_input(&tail) {
            prompt.store(true, Ordering::Relaxed);
        }
    }
}

/// 复核 `output_with_activity` 的收尾路径：tokio 子进程同样被隔离成独立进程组
/// （pgid == pid）。超时或停止时必须整组终止，否则 `sleep 30 & wait` 这类后代
/// 进程会活过截止时间，继续占着输出管道。
#[cfg(unix)]
fn kill_isolated_process_group(pid: u32) {
    if pid == 0 {
        return;
    }
    unsafe {
        libc::kill(-(pid as i32), libc::SIGKILL);
    }
}

#[cfg(not(unix))]
fn kill_isolated_process_group(_pid: u32) {}

/// 执行命令并持续观察输出：识别等待输入的提示符、响应会话级停止请求、
/// 超时或异常时清理整棵进程树。
pub async fn output_with_activity(
    mut command: Command,
    timeout_duration: Duration,
    activity: Option<&ActivityGuard>,
) -> Result<Output, CommandActivityFailure> {
    crate::util::command_env::apply_noninteractive(&mut command);
    command
        .kill_on_drop(true)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    crate::util::process_tree::isolate_std_process_tree(command.as_std_mut());
    let mut child = match command.spawn() {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(CommandActivityFailure::Unavailable)
        }
        Err(_) => return Err(CommandActivityFailure::Failed),
        Ok(child) => child,
    };
    #[cfg(windows)]
    let job = match child.raw_handle().map(attach_job) {
        Some(Ok(job)) => job,
        _ => {
            let _ = child.kill().await;
            return Err(CommandActivityFailure::Failed);
        }
    };
    let stdout = child.stdout.take().ok_or(CommandActivityFailure::Failed)?;
    let stderr = child.stderr.take().ok_or(CommandActivityFailure::Failed)?;
    #[cfg(unix)]
    let child_pid = child.id();
    let prompt = Arc::new(AtomicBool::new(false));
    let id = activity.map(|activity| activity.id.clone());
    let stdout_sink = Arc::new(std::sync::Mutex::new(Vec::new()));
    let stderr_sink = Arc::new(std::sync::Mutex::new(Vec::new()));
    let mut stdout_task = tokio::spawn(capture_stream(
        stdout,
        id.clone(),
        prompt.clone(),
        Arc::clone(&stdout_sink),
    ));
    let mut stderr_task =
        tokio::spawn(capture_stream(stderr, id, prompt.clone(), Arc::clone(&stderr_sink)));
    let deadline = tokio::time::Instant::now() + timeout_duration;
    let mut tick = tokio::time::interval(Duration::from_millis(100));

    let status = loop {
        tokio::select! {
            result = child.wait() => match result {
                Ok(status) => break status,
                Err(_) => {
                    abort_streams(&mut stdout_task, &mut stderr_task);
                    return Err(CommandActivityFailure::Failed);
                }
            },
            _ = tick.tick() => {
                let reason = if prompt.load(Ordering::Relaxed) {
                    Some(CommandActivityFailure::InputRequired)
                } else if activity.is_some_and(ActivityGuard::is_cancelled) {
                    Some(CommandActivityFailure::Stopped)
                } else if tokio::time::Instant::now() >= deadline {
                    Some(CommandActivityFailure::Timeout)
                } else {
                    None
                };
                if let Some(reason) = reason {
                    // 先关 Job Object 再收尾：父进程已退出时子进程仍会被清理。
                    #[cfg(windows)]
                    drop(job);
                    // Unix 侧没有 Job Object：子进程是自己的进程组组长，整组杀掉
                    // 才能连 `sleep 30 &` 这类后代一起终止。
                    #[cfg(unix)]
                    if let Some(pid) = child_pid {
                        kill_isolated_process_group(pid);
                    }
                    let _ = child.start_kill();
                    let _ = child.wait().await;
                    abort_streams(&mut stdout_task, &mut stderr_task);
                    return Err(reason);
                }
            }
        }
    };

    let (stdout, stderr) =
        collect_streams(stdout_task, stderr_task, &stdout_sink, &stderr_sink).await;
    Ok(Output {
        status,
        stdout,
        stderr,
    })
}

/// Shell 退出后，后台后代进程仍可能握着继承来的管道。这里只在宽限期内等 EOF，
/// 超时就中止读端并保留已捕获的内容：既不会把“后台服务占着管道”误报成启动失败，
/// 也不会让工具调用无限等待。
const STREAM_DRAIN_GRACE: Duration = Duration::from_millis(250);

fn abort_streams(
    stdout_task: &mut tokio::task::JoinHandle<std::io::Result<()>>,
    stderr_task: &mut tokio::task::JoinHandle<std::io::Result<()>>,
) {
    stdout_task.abort();
    stderr_task.abort();
}

async fn collect_streams(
    mut stdout_task: tokio::task::JoinHandle<std::io::Result<()>>,
    mut stderr_task: tokio::task::JoinHandle<std::io::Result<()>>,
    stdout_sink: &std::sync::Mutex<Vec<u8>>,
    stderr_sink: &std::sync::Mutex<Vec<u8>>,
) -> (Vec<u8>, Vec<u8>) {
    let drain = async {
        let _ = (&mut stdout_task).await;
        let _ = (&mut stderr_task).await;
    };
    let _ = tokio::time::timeout(STREAM_DRAIN_GRACE, drain).await;
    abort_streams(&mut stdout_task, &mut stderr_task);
    let stdout = stdout_sink.lock().expect("capture sink poisoned").clone();
    let stderr = stderr_sink.lock().expect("capture sink poisoned").clone();
    (stdout, stderr)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shell(command: &str) -> tokio::process::Command {
        let invocation = crate::util::command_env::shell_invocation(command);
        let mut child = tokio::process::Command::new(invocation.program);
        child.args(invocation.args);
        child
    }

    fn decode(bytes: &[u8]) -> String {
        crate::util::command_env::decode_output(bytes)
    }

    /// 每个测试独占一个 session key：`--lib` 用例并行执行，共用字面量会让
    /// “drop 后该 session 为空”的断言与其他用例的在途登记相互干扰。
    fn session_key(label: &str) -> String {
        use std::sync::atomic::AtomicU64;
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let seq = NEXT.fetch_add(1, Ordering::Relaxed);
        format!("{label}-{}-{seq}", std::process::id())
    }

    #[tokio::test]
    async fn m143_interactive_prompt_ends_early_and_next_command_runs() {
        let script = if cfg!(windows) {
            "Write-Output 'Continue? (y/N)'; Start-Sleep -Seconds 30"
        } else {
            "printf 'Continue? (y/N)'; sleep 30"
        };
        let started = std::time::Instant::now();
        let failure = output_with_activity(shell(script), Duration::from_secs(20), None)
            .await
            .unwrap_err();
        assert_eq!(failure, CommandActivityFailure::InputRequired);
        assert!(started.elapsed() < Duration::from_secs(5));

        let output = output_with_activity(shell("echo continued"), Duration::from_secs(10), None)
            .await
            .unwrap();
        assert!(output.status.success());
        assert!(decode(&output.stdout).contains("continued"));
    }

    #[tokio::test]
    async fn m143_activity_tracks_output_and_session_scoped_stop() {
        let session = session_key("activity-tracking");
        let other = session_key("activity-other");
        let activity = track_command(&session, "synthetic command");
        let id = activity.id.clone();
        assert!(command_activity(&other).is_empty());
        assert!(command_activity(&session)
            .iter()
            .any(|item| item.id == id));
        assert!(!stop_command(&other, &id));
        assert!(stop_command(&session, &id));
        assert!(activity.is_cancelled());
        drop(activity);
        assert!(command_activity(&session).is_empty());
    }

    #[tokio::test]
    async fn m143_stop_request_terminates_running_command() {
        let session = session_key("stop-running");
        let activity = track_command(&session, "long sleep");
        let id = activity.id.clone();
        let script = if cfg!(windows) { "Start-Sleep -Seconds 30" } else { "sleep 30" };
        let stopper = {
            let session = session.clone();
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(300)).await;
                stop_command(&session, &id);
            })
        };
        let started = std::time::Instant::now();
        let failure = output_with_activity(shell(script), Duration::from_secs(25), Some(&activity))
            .await
            .unwrap_err();
        stopper.await.unwrap();
        assert_eq!(failure, CommandActivityFailure::Stopped);
        assert!(started.elapsed() < Duration::from_secs(10));
    }
}
