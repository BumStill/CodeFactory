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
            && unsafe { AssignProcessToJobObject(job, child.as_raw_handle() as _) } != 0;
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
    mut command: Command,
    timeout_duration: Duration,
) -> Result<Output, ProcessOutputError> {
    command
        .kill_on_drop(true)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    isolate(&mut command);
    let mut child = match command.spawn() {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(ProcessOutputError::Unavailable)
        }
        Err(_) => return Err(ProcessOutputError::Failed),
        Ok(child) => child,
    };
    let mut stdout = child.stdout.take().ok_or(ProcessOutputError::Failed)?;
    let mut stderr = child.stderr.take().ok_or(ProcessOutputError::Failed)?;
    let mut stdout_task = tokio::spawn(async move {
        let mut bytes = Vec::new();
        stdout.read_to_end(&mut bytes).await.map(|_| bytes)
    });
    let mut stderr_task = tokio::spawn(async move {
        let mut bytes = Vec::new();
        stderr.read_to_end(&mut bytes).await.map(|_| bytes)
    });

    let status = match tokio::time::timeout(timeout_duration, child.wait()).await {
        Ok(Ok(status)) => status,
        Ok(Err(_)) => {
            terminate_and_reap(&mut child, &mut stdout_task, &mut stderr_task).await;
            return Err(ProcessOutputError::Failed);
        }
        Err(_) => {
            terminate_and_reap(&mut child, &mut stdout_task, &mut stderr_task).await;
            return Err(ProcessOutputError::Timeout);
        }
    };

    let (stdout, stderr) = collect_readers(&mut stdout_task, &mut stderr_task).await?;
    Ok(Output {
        status,
        stdout,
        stderr,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::no_window::NoWindow;
    use std::io::Write;
    use std::time::Instant;

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
