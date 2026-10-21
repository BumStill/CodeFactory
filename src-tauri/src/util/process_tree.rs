// SPDX-License-Identifier: Apache-2.0
use std::process::{Output, Stdio};
use std::time::Duration;
use tokio::io::AsyncReadExt;
use tokio::process::{Child, Command};

#[derive(Debug, PartialEq, Eq)]
pub enum ProcessOutputError {
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

fn activities() -> &'static std::sync::Mutex<std::collections::HashMap<String, (CommandActivity, std::sync::Arc<std::sync::atomic::AtomicBool>)>> {
    static ITEMS: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<String, (CommandActivity, std::sync::Arc<std::sync::atomic::AtomicBool>)>>> = std::sync::OnceLock::new();
    ITEMS.get_or_init(Default::default)
}
fn now_sec() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs()
}
pub struct ActivityGuard {
    pub id: String,
    cancelled: std::sync::Arc<std::sync::atomic::AtomicBool>,
}
impl Drop for ActivityGuard {
    fn drop(&mut self) { activities().lock().unwrap().remove(&self.id); }
}
pub fn track_command(session_id: &str, command: &str) -> ActivityGuard {
    let id = uuid::Uuid::new_v4().to_string();
    let cancelled = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    activities().lock().unwrap().insert(id.clone(), (CommandActivity {
        id: id.clone(), session_id: session_id.into(), command: command.into(), last_output_at: now_sec(),
    }, cancelled.clone()));
    ActivityGuard { id, cancelled }
}
pub fn command_activity(session_id: &str) -> Vec<CommandActivity> {
    activities().lock().unwrap().values().filter(|(a,_)| a.session_id == session_id).map(|(a,_)| a.clone()).collect()
}
pub fn stop_command(session_id: &str, id: &str) -> bool {
    let items = activities().lock().unwrap();
    if let Some((activity, stop)) = items.get(id) {
        if activity.session_id == session_id {
            stop.store(true, std::sync::atomic::Ordering::Relaxed);
            return true;
        }
    }
    false
}
fn mark_output(id: Option<&str>) {
    if let Some(id) = id {
        if let Some((item,_)) = activities().lock().unwrap().get_mut(id) { item.last_output_at = now_sec(); }
    }
}

#[cfg(unix)]
pub(crate) fn isolate_std_process_tree(command: &mut std::process::Command) {
    use std::os::unix::process::CommandExt;

    command.process_group(0);
}

#[cfg(not(unix))]
pub(crate) fn isolate_std_process_tree(_command: &mut std::process::Command) {}

pub(crate) struct StdProcessTree {
    #[cfg(unix)]
    process_group_id: i32,
    #[cfg(windows)]
    job: windows_sys::Win32::Foundation::HANDLE,
    #[cfg(not(any(unix, windows)))]
    pid: u32,
}

impl StdProcessTree {
    #[cfg(unix)]
    pub(crate) fn attach(child: &std::process::Child) -> std::io::Result<Self> {
        Ok(Self {
            process_group_id: child.id() as i32,
        })
    }

    #[cfg(windows)]
    pub(crate) fn attach(child: &std::process::Child) -> std::io::Result<Self> {
        use std::os::windows::io::AsRawHandle;
        Self::attach_handle(child.as_raw_handle() as _)
    }

    #[cfg(windows)]
    pub(crate) fn attach_async(child: &Child) -> std::io::Result<Self> {
        Self::attach_handle(child.raw_handle().ok_or_else(|| std::io::Error::other("missing process handle"))? as _)
    }

    #[cfg(windows)]
    fn attach_handle(handle: windows_sys::Win32::Foundation::HANDLE) -> std::io::Result<Self> {
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
            && unsafe { AssignProcessToJobObject(job, handle) } != 0;
        if !assigned {
            let error = std::io::Error::last_os_error();
            unsafe {
                CloseHandle(job);
            }
            return Err(error);
        }
        Ok(Self { job })
    }

    #[cfg(not(any(unix, windows)))]
    pub(crate) fn attach(child: &std::process::Child) -> std::io::Result<Self> {
        Ok(Self { pid: child.id() })
    }

    #[cfg(unix)]
    pub(crate) fn terminate(&self, _child: &mut std::process::Child) -> std::io::Result<()> {
        let result = unsafe { libc::kill(-self.process_group_id, libc::SIGKILL) };
        if result == 0 {
            return Ok(());
        }
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() == Some(libc::ESRCH) {
            Ok(())
        } else {
            Err(error)
        }
    }

    #[cfg(windows)]
    pub(crate) fn terminate(&self, _child: &mut std::process::Child) -> std::io::Result<()> {
        use windows_sys::Win32::System::JobObjects::TerminateJobObject;

        if unsafe { TerminateJobObject(self.job, 1) } != 0 {
            Ok(())
        } else {
            Err(std::io::Error::last_os_error())
        }
    }

    #[cfg(not(any(unix, windows)))]
    pub(crate) fn terminate(&self, child: &mut std::process::Child) -> std::io::Result<()> {
        child.kill()
    }

    #[cfg(unix)]
    pub(crate) fn active_process_count(
        &self,
        _child: &mut std::process::Child,
    ) -> std::io::Result<u32> {
        let result = unsafe { libc::kill(-self.process_group_id, 0) };
        if result == 0 {
            return Ok(1);
        }
        let error = std::io::Error::last_os_error();
        match error.raw_os_error() {
            Some(libc::ESRCH) => Ok(0),
            Some(libc::EPERM) => Ok(1),
            _ => Err(error),
        }
    }

    #[cfg(windows)]
    pub(crate) fn active_process_count(
        &self,
        _child: &mut std::process::Child,
    ) -> std::io::Result<u32> {
        use windows_sys::Win32::System::JobObjects::{
            JobObjectBasicAccountingInformation, QueryInformationJobObject,
            JOBOBJECT_BASIC_ACCOUNTING_INFORMATION,
        };

        let mut accounting = JOBOBJECT_BASIC_ACCOUNTING_INFORMATION::default();
        let queried = unsafe {
            QueryInformationJobObject(
                self.job,
                JobObjectBasicAccountingInformation,
                &mut accounting as *mut _ as *mut std::ffi::c_void,
                std::mem::size_of_val(&accounting) as u32,
                std::ptr::null_mut(),
            )
        };
        if queried != 0 {
            Ok(accounting.ActiveProcesses)
        } else {
            Err(std::io::Error::last_os_error())
        }
    }

    #[cfg(not(any(unix, windows)))]
    pub(crate) fn active_process_count(
        &self,
        child: &mut std::process::Child,
    ) -> std::io::Result<u32> {
        let _ = self.pid;
        Ok(u32::from(child.try_wait()?.is_none()))
    }
}

// Windows handles are kernel references, safe to transfer to the monitor task.
#[cfg(windows)]
unsafe impl Send for StdProcessTree {}

#[cfg(windows)]
impl Drop for StdProcessTree {
    fn drop(&mut self) {
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(self.job);
        }
    }
}

#[cfg(unix)]
fn isolate(command: &mut Command) {
    use std::os::unix::process::CommandExt;

    command.as_std_mut().process_group(0);
}

#[cfg(not(unix))]
fn isolate(_command: &mut Command) {}

#[cfg(unix)]
async fn terminate(child: &mut Child) {
    if let Some(pid) = child.id() {
        unsafe {
            libc::kill(-(pid as i32), libc::SIGKILL);
        }
    }
    let _ = child.start_kill();
}

#[cfg(windows)]
async fn terminate(child: &mut Child) {
    use crate::util::no_window::NoWindow;

    if let Some(pid) = child.id() {
        let mut taskkill = Command::new("taskkill").no_window();
        taskkill.args(["/PID", &pid.to_string(), "/T", "/F"]);
        let _ = tokio::time::timeout(Duration::from_secs(1), taskkill.status()).await;
    }
    let _ = child.start_kill();
}

#[cfg(not(any(unix, windows)))]
async fn terminate(child: &mut Child) {
    let _ = child.start_kill();
}

async fn settle_reader(task: &mut tokio::task::JoinHandle<std::io::Result<Vec<u8>>>) {
    if tokio::time::timeout(Duration::from_millis(500), &mut *task)
        .await
        .is_err()
    {
        task.abort();
        let _ = task.await;
    }
}

async fn collect_readers(
    stdout_task: &mut tokio::task::JoinHandle<std::io::Result<Vec<u8>>>,
    stderr_task: &mut tokio::task::JoinHandle<std::io::Result<Vec<u8>>>,
) -> Result<(Vec<u8>, Vec<u8>), ProcessOutputError> {
    let mut stdout_result = None;
    let mut stderr_result = None;
    let settle_deadline = tokio::time::sleep(Duration::from_millis(500));
    tokio::pin!(settle_deadline);

    while stdout_result.is_none() || stderr_result.is_none() {
        tokio::select! {
            result = &mut *stdout_task, if stdout_result.is_none() => {
                stdout_result = Some(
                    result
                        .map_err(|_| ProcessOutputError::Failed)?
                        .map_err(|_| ProcessOutputError::Failed),
                );
            }
            result = &mut *stderr_task, if stderr_result.is_none() => {
                stderr_result = Some(
                    result
                        .map_err(|_| ProcessOutputError::Failed)?
                        .map_err(|_| ProcessOutputError::Failed),
                );
            }
            _ = &mut settle_deadline => break,
        }
    }

    let capture_incomplete = stdout_result.is_none() || stderr_result.is_none();
    if stdout_result.is_none() {
        stdout_task.abort();
    }
    if stderr_result.is_none() {
        stderr_task.abort();
    }

    let stdout = stdout_result.transpose()?.unwrap_or_default();
    let mut stderr = stderr_result.transpose()?.unwrap_or_default();
    if capture_incomplete {
        stderr.extend_from_slice(
            b"[output capture stopped because a background process retained the pipe]\n",
        );
    }
    Ok((stdout, stderr))
}

async fn terminate_and_reap(
    child: &mut Child,
    stdout_task: &mut tokio::task::JoinHandle<std::io::Result<Vec<u8>>>,
    stderr_task: &mut tokio::task::JoinHandle<std::io::Result<Vec<u8>>>,
) {
    terminate(child).await;
    let _ = tokio::time::timeout(Duration::from_secs(1), child.wait()).await;
    settle_reader(stdout_task).await;
    settle_reader(stderr_task).await;
}

pub async fn output_with_timeout(
    command: Command,
    timeout_duration: Duration,
) -> Result<Output, ProcessOutputError> {
    output_with_activity(command, timeout_duration, None).await
}

async fn capture_stream<R: tokio::io::AsyncRead + Unpin>(mut reader: R, id: Option<String>, prompt: std::sync::Arc<std::sync::atomic::AtomicBool>) -> std::io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        let count = reader.read(&mut chunk).await?;
        if count == 0 { return Ok(bytes); }
        bytes.extend_from_slice(&chunk[..count]);
        mark_output(id.as_deref());
        let start = bytes.len().saturating_sub(4096);
        if super::command_env::waiting_for_input(&super::command_env::decode_output(&bytes[start..])) {
            prompt.store(true, std::sync::atomic::Ordering::Relaxed);
        }
    }
}

pub async fn output_with_activity(
    mut command: Command,
    timeout_duration: Duration,
    activity: Option<&ActivityGuard>,
) -> Result<Output, ProcessOutputError> {
    super::command_env::apply_noninteractive(&mut command);
    command.kill_on_drop(true).stdout(Stdio::piped()).stderr(Stdio::piped());
    isolate(&mut command);
    let mut child = match command.spawn() {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Err(ProcessOutputError::Unavailable),
        Err(_) => return Err(ProcessOutputError::Failed),
        Ok(child) => child,
    };
    #[cfg(windows)]
    let _job = match StdProcessTree::attach_async(&child) {
        Ok(job) => job,
        Err(_) => { let _ = child.kill().await; return Err(ProcessOutputError::Failed); }
    };
    let stdout = child.stdout.take().ok_or(ProcessOutputError::Failed)?;
    let stderr = child.stderr.take().ok_or(ProcessOutputError::Failed)?;
    let prompt = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let id = activity.map(|a| a.id.clone());
    let mut stdout_task = tokio::spawn(capture_stream(stdout, id.clone(), prompt.clone()));
    let mut stderr_task = tokio::spawn(capture_stream(stderr, id, prompt.clone()));
    let deadline = tokio::time::Instant::now() + timeout_duration;
    let mut tick = tokio::time::interval(Duration::from_millis(100));
    let status = loop {
        tokio::select! {
            result = child.wait() => match result {
                Ok(status) => break status,
                Err(_) => {
                    terminate_and_reap(&mut child, &mut stdout_task, &mut stderr_task).await;
                    return Err(ProcessOutputError::Failed);
                }
            },
            _ = tick.tick() => {
                let reason = if prompt.load(std::sync::atomic::Ordering::Relaxed) { Some(ProcessOutputError::InputRequired) }
                    else if activity.is_some_and(|a| a.cancelled.load(std::sync::atomic::Ordering::Relaxed)) { Some(ProcessOutputError::Stopped) }
                    else if tokio::time::Instant::now() >= deadline { Some(ProcessOutputError::Timeout) } else { None };
                if let Some(reason) = reason {
                    // Closing the Windows job kills descendants even if their parent exited.
                    #[cfg(windows)]
                    drop(_job);
                    terminate_and_reap(&mut child, &mut stdout_task, &mut stderr_task).await;
                    return Err(reason);
                }
            }
        }
    };
    let (stdout, stderr) = collect_readers(&mut stdout_task, &mut stderr_task).await?;
    Ok(Output { status, stdout, stderr })
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::no_window::NoWindow;
    use std::io::Write;
    use std::time::Instant;

    #[tokio::test]
    async fn m143_interactive_prompt_ends_early_and_next_command_runs() {
        let shell = crate::util::command_env::shell_invocation(if cfg!(windows) {
            "Write-Output 'Continue? (y/N)'; Start-Sleep -Seconds 30"
        } else { "printf 'Continue? (y/N)'; sleep 30" });
        let mut command = Command::new(shell.program);
        command.args(shell.args);
        let started = Instant::now();
        assert_eq!(output_with_timeout(command, Duration::from_secs(20)).await.unwrap_err(), ProcessOutputError::InputRequired);
        assert!(started.elapsed() < Duration::from_secs(5));
        let shell = crate::util::command_env::shell_invocation("echo continued");
        let mut command = Command::new(shell.program);
        command.args(shell.args);
        let output = output_with_timeout(command, Duration::from_secs(10)).await.unwrap();
        assert!(output.status.success());
        assert!(crate::util::command_env::decode_output(&output.stdout).contains("continued"));
    }

    #[tokio::test]
    async fn m143_activity_tracks_output_and_session_scoped_stop() {
        let activity = track_command("synthetic-session", "synthetic command");
        let id = activity.id.clone();
        assert_eq!(command_activity("other-session").len(), 0);
        assert!(command_activity("synthetic-session").iter().any(|item| item.id == id));
        assert!(!stop_command("other-session", &id));
        assert!(stop_command("synthetic-session", &id));
        assert!(activity.cancelled.load(std::sync::atomic::Ordering::Relaxed));
        drop(activity);
        assert!(command_activity("synthetic-session").is_empty());
    }

    fn wait_for_parent(child: &mut std::process::Child, timeout: Duration) {
        let deadline = Instant::now() + timeout;
        loop {
            if child.try_wait().expect("poll parent").is_some() {
                return;
            }
            assert!(Instant::now() < deadline, "fixture parent did not exit");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn wait_for_pid(path: &std::path::Path, timeout: Duration) -> u32 {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(pid) = std::fs::read_to_string(path)
                .ok()
                .and_then(|value| value.trim().parse::<u32>().ok())
            {
                return pid;
            }
            assert!(
                Instant::now() < deadline,
                "fixture descendant pid was not recorded"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[cfg(unix)]
    fn detached_descendant_command(pid_path: &std::path::Path) -> std::process::Command {
        let mut command = std::process::Command::new("sh").no_window();
        command.args([
            "-c",
            &format!(
                "read gate; sleep 30 >/dev/null 2>&1 & echo $! > '{}'",
                pid_path.display()
            ),
        ]);
        command
    }

    #[cfg(windows)]
    fn detached_descendant_command(pid_path: &std::path::Path) -> std::process::Command {
        let escaped = pid_path.display().to_string().replace('\'', "''");
        let script = format!(
            "$null = [Console]::In.ReadLine(); \
             $p = Start-Process powershell -ArgumentList '-NoProfile','-Command','Start-Sleep -Seconds 30' -PassThru; \
             Set-Content -LiteralPath '{escaped}' -Value $p.Id"
        );
        let mut command = std::process::Command::new("powershell").no_window();
        command.args(["-NoProfile", "-Command", &script]);
        command
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn std_process_tree_sweep_reaps_a_detached_descendant() {
        let fixture = tempfile::tempdir().expect("fixture dir");
        let pid_path = fixture.path().join("descendant.pid");
        let mut command = detached_descendant_command(&pid_path);
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        isolate_std_process_tree(&mut command);
        let mut child = command.spawn().expect("spawn fixture parent");
        let tree = StdProcessTree::attach(&child).expect("attach process tree");
        child
            .stdin
            .take()
            .expect("fixture start gate")
            .write_all(b"start\n")
            .expect("release fixture start gate");

        wait_for_parent(&mut child, Duration::from_secs(5));
        let descendant_pid = wait_for_pid(&pid_path, Duration::from_secs(5));
        assert_ne!(descendant_pid, child.id());
        assert!(
            tree.active_process_count(&mut child).expect("count tree") > 0,
            "fixture must leave a live detached descendant before the sweep"
        );

        tree.terminate(&mut child).expect("terminate process tree");
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            if tree.active_process_count(&mut child).expect("recount tree") == 0 {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "detached descendant survived tree sweep"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}
